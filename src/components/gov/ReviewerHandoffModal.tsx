"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import {
  ShieldAlert,
  UserCheck,
  CheckCircle2,
  AlertTriangle,
  X,
  ArrowRight,
  ShieldCheck,
  Calendar,
} from "lucide-react";

interface ReviewerHandoffModalProps {
  isOpen: boolean;
  onClose: () => void;
  preselectedDocCode?: string;
}

export function ReviewerHandoffModal({
  isOpen,
  onClose,
  preselectedDocCode,
}: ReviewerHandoffModalProps) {
  const approvals = useAppStore((s) => s.approvals);
  const employees = useAppStore((s) => s.employees);
  const reassignApprovalDelegate = useAppStore((s) => s.reassignApprovalDelegate);

  // Available pending approvals
  const pendingApprovals = approvals.filter((a) =>
    a.stages.some((st) => st.status === "pending")
  );

  const [selectedDocCode, setSelectedDocCode] = useState(
    preselectedDocCode || pendingApprovals[0]?.code || "AP-3121"
  );
  const [targetEmployeeId, setTargetEmployeeId] = useState(employees[1]?.id || "");
  const [reason, setReason] = useState("정기 연차 및 해외 출장에 따른 직무 대결 지정");

  if (!isOpen) return null;

  const currentDoc = approvals.find((a) => a.code === selectedDocCode);
  const pendingStage = currentDoc?.stages.find((st) => st.status === "pending");
  const targetEmployee = employees.find((e) => e.id === targetEmployeeId);

  // Independence check: Is target employee the drafter?
  const isDrafterViolation =
    currentDoc &&
    targetEmployee &&
    (currentDoc.drafterId === targetEmployee.id ||
      currentDoc.drafterName === targetEmployee.name);

  const handleExecuteHandoff = () => {
    if (!currentDoc || !targetEmployee) return;
    const result = reassignApprovalDelegate(
      currentDoc.code,
      pendingStage?.approverName || "기존 결재자",
      targetEmployee.id,
      reason
    );
    if (result.success) {
      onClose();
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 backdrop-blur-xs flex items-center justify-center z-50 p-4">
      <div className="bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-2xl max-w-xl w-full p-6 space-y-5 animate-in fade-in zoom-in-95 duration-150">
        {/* 헤더 */}
        <div className="flex items-start justify-between border-b border-[var(--border)] pb-3">
          <div className="flex items-center gap-2.5">
            <div className="w-9 h-9 rounded-lg bg-amber-500/10 border border-amber-500/20 text-amber-700 flex items-center justify-center">
              <UserCheck className="w-5 h-5" />
            </div>
            <div>
              <h3 className="font-bold text-base text-[var(--ink)]">
                부재자 결재 대결(Delegate) 및 직무 인수인계
              </h3>
              <p className="text-xs text-[var(--steel)]">
                장기 부재 결재권자의 권한을 인계하며, 직무 분리(SoD) 규정에 따른 독립성을 실시간 검증합니다.
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded-md text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* 폼 본문 */}
        <div className="space-y-4 text-xs">
          {/* 1. 대상 결재 문서 선택 */}
          <div>
            <label className="font-bold block mb-1 text-[var(--ink)]">대결 처리할 결재 문서 (AP-*)</label>
            <select
              value={selectedDocCode}
              onChange={(e) => setSelectedDocCode(e.target.value)}
              className="w-full h-9 px-3 rounded-md border border-[var(--border)] bg-[var(--surface)] font-medium"
            >
              {pendingApprovals.map((doc) => (
                <option key={doc.id} value={doc.code}>
                  [{doc.code}] {doc.title} (기안: {doc.drafterName} · 현재 대기: {doc.stages.find((s) => s.status === "pending")?.approverName})
                </option>
              ))}
            </select>
          </div>

          {/* 2. 결재선 인계 흐름 시각화 */}
          {currentDoc && (
            <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-3 space-y-2">
              <div className="flex items-center justify-between text-[11px] text-[var(--steel)]">
                <span>문서 기안자: <strong className="text-[var(--ink)]">{currentDoc.drafterName}</strong> ({currentDoc.drafterDept})</span>
                <span>품의 구분: <strong className="text-[var(--ink)]">{currentDoc.category}</strong></span>
              </div>
              <div className="flex items-center gap-2 pt-1 border-t border-[var(--border)]/60">
                <div className="flex-1 p-2 rounded border border-[var(--border)] bg-[var(--surface)] text-center">
                  <div className="text-[10px] text-[var(--steel)]">현재 대기 결재자</div>
                  <div className="font-bold text-xs text-[var(--ink)] truncate">
                    {pendingStage?.approverName || "부재 결재자"}
                  </div>
                </div>
                <ArrowRight className="w-4 h-4 text-[var(--steel)] shrink-0" />
                <div className="flex-1 p-2 rounded border border-indigo-200 bg-indigo-50/50 text-center">
                  <div className="text-[10px] text-indigo-700 font-semibold">인계받을 대결권자</div>
                  <div className="font-bold text-xs text-indigo-950 truncate">
                    {targetEmployee?.name || "선택 필요"} ({targetEmployee?.role || "직책"})
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* 3. 대결자 선택 */}
          <div>
            <label className="font-bold block mb-1 text-[var(--ink)]">인계 대상 임직원 (대결 후보자)</label>
            <select
              value={targetEmployeeId}
              onChange={(e) => setTargetEmployeeId(e.target.value)}
              className="w-full h-9 px-3 rounded-md border border-[var(--border)] bg-[var(--surface)] font-medium"
            >
              {employees.map((emp) => (
                <option key={emp.id} value={emp.id}>
                  {emp.name} ({emp.dept} · {emp.role} · {emp.entity})
                </option>
              ))}
            </select>
          </div>

          {/* 4. 직무 분리 (SoD) 실시간 독립성 검증 배너 */}
          {isDrafterViolation ? (
            <div className="rounded-lg border border-red-300 bg-red-50 p-3.5 space-y-1.5 animate-in fade-in duration-150">
              <div className="flex items-center gap-2 text-red-800 font-bold">
                <AlertTriangle className="w-4 h-4 text-red-600" />
                <span>직무 분리(SoD) 규정 위반 — 대결 지정 불가</span>
              </div>
              <p className="text-[11px] text-red-700 leading-relaxed">
                선택한 <strong>{targetEmployee?.name}</strong> 님은 해당 문서의 <strong>최초 기안자</strong>입니다.
                내부통제 및 근로기준법 사규에 따라 자가 승인은 엄격히 금지되며, 대결권을 위임할 수 없습니다.
              </p>
            </div>
          ) : (
            <div className="rounded-lg border border-emerald-300 bg-emerald-50 p-3.5 space-y-1.5 animate-in fade-in duration-150">
              <div className="flex items-center gap-2 text-emerald-800 font-bold">
                <ShieldCheck className="w-4 h-4 text-emerald-600" />
                <span>상호 견제 및 독립성 검증 통과 (SoD Verified)</span>
              </div>
              <p className="text-[11px] text-emerald-700 leading-relaxed">
                기안자(<strong>{currentDoc?.drafterName}</strong>)와 대결자(<strong>{targetEmployee?.name}</strong>)가 상이한 자연인(Person)으로 확인되었습니다.
                결재 이력이 감사 원장에 영구 보존됩니다.
              </p>
            </div>
          )}

          {/* 5. 대결 사유 및 인수인계 근거 */}
          <div>
            <label className="font-bold block mb-1 text-[var(--ink)]">인수인계 및 대결 지정 사유</label>
            <input
              type="text"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="예: 2026-07-25~07-30 하계 휴가로 인한 부서장 직무 대결"
              className="w-full h-9 px-3 rounded-md border border-[var(--border)] bg-[var(--surface)] text-xs"
            />
          </div>
        </div>

        {/* 하단 액션 버튼 */}
        <div className="flex items-center justify-between pt-3 border-t border-[var(--border)]">
          <span className="text-[11px] text-[var(--steel)]">
            대결권 부여 시 결재 문서에 [대결] 표기가 자동으로 부여됩니다.
          </span>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="ghost" onClick={onClose}>
              취소
            </Button>
            <Button
              size="sm"
              variant="brand"
              disabled={isDrafterViolation || !targetEmployee}
              onClick={handleExecuteHandoff}
              leftIcon={<CheckCircle2 className="w-4 h-4" />}
            >
              대결 지정 확정
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
