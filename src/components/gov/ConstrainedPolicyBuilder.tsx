"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { CAPABILITY_BUNDLES, previewPolicyChange } from "@/lib/policy-engine";
import { AccessGrantAssignment } from "@/lib/types";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import {
  ShieldPlus,
  ShieldAlert,
  Users,
  Building,
  Calendar,
  FileCheck2,
  Lock,
  Trash2,
  AlertTriangle,
  CheckCircle2,
  X,
  KeyRound,
  Info,
} from "lucide-react";

export function ConstrainedPolicyBuilder() {
  const employees = useAppStore((s) => s.employees);
  const orgEntities = useAppStore((s) => s.orgEntities);
  const accessAssignments = useAppStore((s) => s.accessAssignments);
  const grantAccessAssignment = useAppStore((s) => s.grantAccessAssignment);
  const revokeAccessAssignment = useAppStore((s) => s.revokeAccessAssignment);
  const addToast = useAppStore((s) => s.addToast);

  // Form state
  const [assigneeType, setAssigneeType] = useState<"account" | "role_group">("account");
  const [selectedAccountId, setSelectedAccountId] = useState(employees[0]?.id || "");
  const [selectedRoleGroup, setSelectedRoleGroup] = useState("팀장");
  const [selectedBundleId, setSelectedBundleId] = useState(CAPABILITY_BUNDLES[0]?.id || "cap-hr-onboard");
  const [entityScope, setEntityScope] = useState("all");
  const [validFrom, setValidFrom] = useState(new Date().toISOString().slice(0, 10));
  const [validTo, setValidTo] = useState("2026-12-31");
  const [justification, setJustification] = useState("");

  // Pre-commit Preview Modal state
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewData, setPreviewData] = useState<ReturnType<typeof previewPolicyChange> | null>(null);

  const selectedBundle = CAPABILITY_BUNDLES.find((b) => b.id === selectedBundleId);

  const handleOpenPreview = (e: React.FormEvent) => {
    e.preventDefault();
    if (!justification.trim()) {
      addToast({
        title: "직무 사유 필수 입력",
        description: "권한 배정을 위해서는 내부통제 감사 사유(Justification)를 필수로 기재해야 합니다.",
        tone: "warn",
      });
      return;
    }

    const targetId = assigneeType === "account" ? selectedAccountId : selectedRoleGroup;
    const diff = previewPolicyChange(
      assigneeType,
      targetId,
      selectedBundleId,
      entityScope,
      employees,
      accessAssignments
    );
    setPreviewData(diff);
    setPreviewOpen(true);
  };

  const handleConfirmGrant = () => {
    const assigneeName =
      assigneeType === "account"
        ? employees.find((e) => e.id === selectedAccountId)?.name + ` (${selectedAccountId})`
        : `${selectedRoleGroup} 직무 그룹`;

    const personId =
      assigneeType === "account"
        ? `P-${selectedAccountId}`
        : undefined;

    grantAccessAssignment({
      assigneeType,
      assigneeId: assigneeType === "account" ? selectedAccountId : selectedRoleGroup,
      assigneeName,
      personId,
      capabilityBundleId: selectedBundleId,
      capabilityBundleName: selectedBundle?.name || selectedBundleId,
      entityScope,
      validFrom,
      validTo,
      justification,
      grantedBy: "ACC-SYS-ADMIN",
    });

    setPreviewOpen(false);
    setJustification("");
  };

  return (
    <div className="space-y-6">
      {/* 1. Constrained Policy Assignment Form */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4">
        <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
          <div className="flex items-center gap-2">
            <ShieldPlus className="w-5 h-5 text-indigo-600" />
            <div>
              <h3 className="font-bold text-sm text-[var(--ink)]">
                비기술 직무 권한 배정기 (Constrained Capability Builder)
              </h3>
              <span className="text-[11px] text-[var(--steel)]">
                복잡한 Cedar DSL 코드 대신 직무 번들, 관할 법인, 유효 기간을 선택하여 안전하게 권한을 배정합니다.
              </span>
            </div>
          </div>
          <StatusChip label="내부통제 표준" tone="info" size="xs" />
        </div>

        <form onSubmit={handleOpenPreview} className="space-y-4 text-xs">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            {/* 권한 대상 유형 */}
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">권한 대상 유형</label>
              <div className="grid grid-cols-2 gap-2">
                <button
                  type="button"
                  onClick={() => setAssigneeType("account")}
                  className={`h-8 rounded border text-xs font-semibold cursor-pointer transition ${
                    assigneeType === "account"
                      ? "border-indigo-600 bg-indigo-50 text-indigo-900"
                      : "border-[var(--border)] bg-[var(--surface)] text-[var(--steel)]"
                  }`}
                >
                  임직원 개인 계정
                </button>
                <button
                  type="button"
                  onClick={() => setAssigneeType("role_group")}
                  className={`h-8 rounded border text-xs font-semibold cursor-pointer transition ${
                    assigneeType === "role_group"
                      ? "border-indigo-600 bg-indigo-50 text-indigo-900"
                      : "border-[var(--border)] bg-[var(--surface)] text-[var(--steel)]"
                  }`}
                >
                  직무 역할 그룹
                </button>
              </div>
            </div>

            {/* 대상자 선택 */}
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">
                {assigneeType === "account" ? "대상 임직원 선택" : "직무 역할 선택"}
              </label>
              {assigneeType === "account" ? (
                <select
                  value={selectedAccountId}
                  onChange={(e) => setSelectedAccountId(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-xs"
                >
                  {employees.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.name} ({e.dept} · {e.role} · {e.entity})
                    </option>
                  ))}
                </select>
              ) : (
                <select
                  value={selectedRoleGroup}
                  onChange={(e) => setSelectedRoleGroup(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-xs"
                >
                  <option value="팀장">팀장 / 부서장 그룹 (DoA 1단계)</option>
                  <option value="총괄본부장">총괄본부장 C-Level 그룹 (DoA 2단계)</option>
                  <option value="대표이사">대표이사 / 회장단 (DoA 3단계)</option>
                  <option value="인사담당">인사총무 실무담당자 그룹</option>
                  <option value="현장소장">현장 사업소장 / 엔지니어 관리자</option>
                </select>
              )}
            </div>

            {/* 관할 법인 (Tenant Boundary) */}
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">관할 법인 경계 (Entity Scope)</label>
              <select
                value={entityScope}
                onChange={(e) => setEntityScope(e.target.value)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-xs font-medium"
              >
                <option value="all">그룹 전사 통합 (All Entities)</option>
                {orgEntities.map((ent) => (
                  <option key={ent.id} value={ent.id}>
                    {ent.name} ({ent.code})
                  </option>
                ))}
              </select>
            </div>
          </div>

          {/* 직무 권한 번들 (Capability Bundle) */}
          <div>
            <label className="font-bold block mb-1.5 text-[var(--ink)]">직무 권한 번들 (Semantic Capability)</label>
            <div className="grid grid-cols-1 md:grid-cols-3 gap-2.5">
              {CAPABILITY_BUNDLES.map((bundle) => {
                const isSelected = selectedBundleId === bundle.id;
                return (
                  <div
                    key={bundle.id}
                    onClick={() => setSelectedBundleId(bundle.id)}
                    className={`p-3 rounded-lg border text-left cursor-pointer transition space-y-1.5 ${
                      isSelected
                        ? "border-indigo-600 bg-indigo-50/50 shadow-2xs"
                        : "border-[var(--border)] bg-[var(--surface)] hover:border-slate-400"
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-bold text-xs text-[var(--ink)]">{bundle.name}</span>
                      <StatusChip label={bundle.category} size="xs" tone={isSelected ? "brand" : "neutral"} />
                    </div>
                    <p className="text-[11px] text-[var(--steel)] leading-snug">{bundle.description}</p>
                    <div className="flex items-center gap-2 pt-1 text-[10px] text-[var(--faint)]">
                      <span>전결 기준: <strong>{bundle.suggestedDoATier}</strong></span>
                      {bundle.requiresPasskeyDefault && (
                        <span className="text-purple-700 font-bold flex items-center gap-0.5">
                          <KeyRound className="w-2.5 h-2.5" /> Passkey 필수
                        </span>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>

          {/* 유효 기간 및 사유 */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4 pt-2 border-t border-[var(--border)]/60">
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">유효 시작일</label>
              <input
                type="date"
                value={validFrom}
                onChange={(e) => setValidFrom(e.target.value)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
            </div>
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">만료일 (Validity Expiration)</label>
              <input
                type="date"
                value={validTo}
                onChange={(e) => setValidTo(e.target.value)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
            </div>
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">
                내부통제 배정 사유 (Justification) <span className="text-red-500">*</span>
              </label>
              <input
                type="text"
                value={justification}
                onChange={(e) => setJustification(e.target.value)}
                placeholder="예: 2026 하반기 조직개편에 따른 팀장 직결권 인가"
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
              />
            </div>
          </div>

          {/* 제출 버튼 */}
          <div className="flex items-center justify-between pt-3 border-t border-[var(--border)]">
            <span className="text-[11px] text-[var(--steel)] flex items-center gap-1.5">
              <Info className="w-4 h-4 text-indigo-600" />
              <span>권한 확정 전 사전 영향도 검토(Pre-commit Diff) 모달이 열립니다.</span>
            </span>
            <Button size="sm" variant="brand" type="submit" leftIcon={<FileCheck2 className="w-4 h-4" />}>
              권한 변경 사전 영향도 검토 (Pre-commit Preview)
            </Button>
          </div>
        </form>
      </div>

      {/* 2. 현재 활성 직무 권한 배정 원장 (Active Assignments Table) */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-3">
        <div className="flex items-center justify-between border-b border-[var(--border)] pb-2.5">
          <div>
            <h4 className="font-bold text-xs text-[var(--ink)] flex items-center gap-2">
              <Users className="w-4 h-4 text-emerald-600" />
              <span>활성 직무 권한 배정 대장 (Active Access Grants)</span>
            </h4>
            <span className="text-[11px] text-[var(--steel)]">
              임직원 및 역할 그룹에 부여된 유효 권한 목록입니다. 만료 시 자동으로 Default-Deny 상태로 복귀합니다.
            </span>
          </div>
          <span className="text-xs font-mono text-[var(--steel)]">
            총 {accessAssignments.filter((a) => a.status === "active").length}건 활성
          </span>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs border-collapse">
            <thead>
              <tr className="border-b border-[var(--border)] bg-[var(--canvas)] text-[var(--steel)] text-[11px]">
                <th className="py-2 px-3">권한 대상자</th>
                <th className="py-2 px-3">부여된 직무 번들</th>
                <th className="py-2 px-3">관할 법인</th>
                <th className="py-2 px-3">유효 기간</th>
                <th className="py-2 px-3">배정 사유 및 근거</th>
                <th className="py-2 px-3 text-center">상태</th>
                <th className="py-2 px-3 text-right">관리</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border)]">
              {accessAssignments.map((grant) => (
                <tr key={grant.id} className="hover:bg-[var(--canvas)] transition">
                  <td className="py-2.5 px-3 font-bold text-[var(--ink)]">
                    <div className="flex items-center gap-1.5">
                      <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
                      <span>{grant.assigneeName}</span>
                    </div>
                  </td>
                  <td className="py-2.5 px-3 font-semibold text-indigo-900">
                    {grant.capabilityBundleName}
                  </td>
                  <td className="py-2.5 px-3 font-mono text-[11px] text-[var(--steel)]">
                    {grant.entityScope === "all" ? "전사 통합" : grant.entityScope}
                  </td>
                  <td className="py-2.5 px-3 font-mono text-[11px] text-[var(--steel)]">
                    {grant.validFrom} ~ {grant.validTo}
                  </td>
                  <td className="py-2.5 px-3 text-[var(--ink)] max-w-xs truncate">
                    {grant.justification}
                  </td>
                  <td className="py-2.5 px-3 text-center">
                    <StatusChip
                      label={grant.status === "active" ? "활성" : "회수됨"}
                      tone={grant.status === "active" ? "ok" : "danger"}
                      size="xs"
                    />
                  </td>
                  <td className="py-2.5 px-3 text-right">
                    {grant.status === "active" && (
                      <button
                        onClick={() => revokeAccessAssignment(grant.id, "관리자 직권 회수")}
                        className="text-[11px] text-red-600 hover:text-red-800 font-semibold cursor-pointer"
                      >
                        권한 회수
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* 3. Pre-Commit Policy Change Preview Modal */}
      {previewOpen && previewData && (
        <div className="fixed inset-0 bg-black/50 backdrop-blur-xs flex items-center justify-center z-50 p-4">
          <div className="bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-2xl max-w-lg w-full p-6 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-start justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <ShieldAlert className="w-5 h-5 text-indigo-600" />
                <h3 className="font-bold text-sm text-[var(--ink)]">
                  권한 변경 사전 영향도 검토 (Pre-commit Diff)
                </h3>
              </div>
              <button
                onClick={() => setPreviewOpen(false)}
                className="p-1 rounded text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <X className="w-5 h-5" />
              </button>
            </div>

            <div className="space-y-3 text-xs">
              <div className="p-3 rounded-lg border border-[var(--border)] bg-[var(--canvas)] space-y-1.5">
                <div className="text-[11px] text-[var(--steel)]">부여할 직무 번들</div>
                <div className="font-bold text-sm text-indigo-950">{previewData.bundleName}</div>
                <div className="text-[11px] text-[var(--steel)]">
                  관할 범위: <strong>{entityScope === "all" ? "그룹 전사 통합 (All Entities)" : entityScope}</strong> · 만료: <strong>{validTo}</strong>
                </div>
              </div>

              {/* 신규 권한 획득 임직원 */}
              <div>
                <label className="font-bold block mb-1 text-emerald-800 flex items-center gap-1.5">
                  <CheckCircle2 className="w-4 h-4 text-emerald-600" />
                  <span>신규 권한 획득 대상자 ({previewData.gainedUsers.length}명)</span>
                </label>
                <div className="max-h-28 overflow-y-auto p-2 rounded border border-[var(--border)] bg-[var(--surface)] space-y-1">
                  {previewData.gainedUsers.map((u, idx) => (
                    <div key={idx} className="text-[11px] text-[var(--ink)] font-medium">
                      • {u}
                    </div>
                  ))}
                </div>
              </div>

              {/* 내부통제 위험 경고 */}
              {previewData.riskWarnings.length > 0 && (
                <div className="rounded-lg border border-amber-300 bg-amber-50 p-3 space-y-1 text-amber-900">
                  <div className="font-bold flex items-center gap-1.5 text-xs">
                    <AlertTriangle className="w-4 h-4 text-amber-700" />
                    <span>내부회계관리제도 위험 안내</span>
                  </div>
                  <ul className="list-disc list-inside space-y-0.5 text-[11px]">
                    {previewData.riskWarnings.map((w, idx) => (
                      <li key={idx}>{w}</li>
                    ))}
                  </ul>
                </div>
              )}
            </div>

            <div className="flex items-center justify-between pt-3 border-t border-[var(--border)]">
              <span className="text-[10px] text-[var(--steel)]">감사 로그에 승인자 계정이 영구 기록됩니다.</span>
              <div className="flex items-center gap-2">
                <Button size="sm" variant="ghost" onClick={() => setPreviewOpen(false)}>
                  취소
                </Button>
                <Button size="sm" variant="brand" onClick={handleConfirmGrant} leftIcon={<CheckCircle2 className="w-4 h-4" />}>
                  영향도 확인 및 권한 확정 부여
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
