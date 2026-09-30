"use client";

import React, { useState, useMemo } from "react";
import { useAppStore } from "@/lib/store";
import { ApprovalDoc, EffectivePermissionFold } from "@/lib/types";
import { Button } from "@/components/ui/Button";
import {
  Fingerprint,
  ShieldCheck,
  AlertCircle,
  FileCheck2,
  CheckCircle2,
  Lock,
  X,
} from "lucide-react";

interface CapacitySignaturePickerProps {
  doc: ApprovalDoc;
  approverId: string;
  approverName: string;
  isOpen: boolean;
  onClose: () => void;
  onSign: (signature: {
    actingCapacityRole: string;
    actingCapacityName: string;
    authorizingGrantId?: string;
    passkeyVerified: boolean;
    comment: string;
  }) => void;
}

export function CapacitySignaturePicker({
  doc,
  approverId,
  approverName,
  isOpen,
  onClose,
  onSign,
}: CapacitySignaturePickerProps) {
  const getEffectivePermissionsForEmployee = useAppStore(
    (s) => s.getEffectivePermissionsForEmployee
  );
  const roleAssignments = useAppStore((s) => s.roleAssignments);

  const [comment, setComment] = useState("");
  const [selectedRoleId, setSelectedRoleId] = useState<string>("");
  const [passkeyState, setPasskeyState] = useState<"idle" | "verifying" | "verified">("idle");
  const [passkeyError, setPasskeyError] = useState<string | null>(null);

  // Compute approver's effective capacities
  const effectiveFold: EffectivePermissionFold = useMemo(() => {
    return getEffectivePermissionsForEmployee(approverId);
  }, [approverId, getEffectivePermissionsForEmployee, roleAssignments]);

  const capacities = effectiveFold.actingCapacities;

  // Initialize selected role if empty
  React.useEffect(() => {
    if (capacities.length > 0 && !selectedRoleId) {
      setSelectedRoleId(capacities[0].roleId);
    }
  }, [capacities, selectedRoleId]);

  if (!isOpen) return null;

  const activeCapacity = capacities.find((c) => c.roleId === selectedRoleId);

  // DoA Amount Check
  const docAmount = doc.doaAmount || 0;
  const isOverLimit =
    activeCapacity?.maxAmount !== undefined && docAmount > activeCapacity.maxAmount;

  const requiresPasskey = !!activeCapacity?.requiresPasskey;

  // Simulate Passkey WebAuthn biometric assertion
  const handlePasskeyAuth = () => {
    setPasskeyState("verifying");
    setPasskeyError(null);
    setTimeout(() => {
      setPasskeyState("verified");
    }, 800);
  };

  const handleConfirm = () => {
    if (!activeCapacity) return;
    if (isOverLimit) return;
    if (requiresPasskey && passkeyState !== "verified") {
      setPasskeyError("고위험 자격 서명을 위해 하드웨어 Passkey 생체 인증을 완료해야 합니다.");
      return;
    }

    onSign({
      actingCapacityRole: activeCapacity.roleId,
      actingCapacityName: activeCapacity.roleName,
      authorizingGrantId: `grant-${activeCapacity.roleId}`,
      passkeyVerified: requiresPasskey && passkeyState === "verified",
      comment: comment || "원안 승인 (규정 적합)",
    });

    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4 animate-in fade-in duration-150">
      <div className="bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-2xl w-full max-w-lg overflow-hidden flex flex-col">
        {/* Header */}
        <div className="px-5 py-4 border-b border-[var(--border)] flex items-center justify-between bg-gradient-to-r from-indigo-900/10 via-transparent to-transparent">
          <div className="flex items-center gap-2">
            <span className="p-1.5 rounded-lg bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
              <FileCheck2 size={18} />
            </span>
            <div>
              <h3 className="text-sm font-bold text-[var(--ink)]">
                직무 전결 자격 기반 전자서명 (전자서명법 제3조)
              </h3>
              <p className="text-[11px] text-[var(--steel)]">
                {doc.code} — {doc.title}
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded-md text-[var(--steel)] hover:text-[var(--ink)] hover:bg-[var(--muted)]"
          >
            <X size={16} />
          </button>
        </div>

        {/* Content */}
        <div className="p-5 space-y-4 text-xs overflow-y-auto max-h-[75vh]">
          {/* Signer Info */}
          <div className="p-3 rounded-lg bg-[var(--muted)]/50 border border-[var(--border)] flex items-center justify-between">
            <div>
              <span className="text-[11px] text-[var(--steel)] block">서명자 (Signer)</span>
              <span className="font-semibold text-sm text-[var(--ink)]">
                {approverName}
              </span>
            </div>
            <div className="text-right">
              <span className="text-[11px] text-[var(--steel)] block">결재 품의 금액</span>
              <span className="font-mono font-bold text-sm text-indigo-600 dark:text-indigo-400">
                ₩{docAmount.toLocaleString()}
              </span>
            </div>
          </div>

          {/* 직무 전결 자격 및 인가 범위 선택 */}
          <div className="space-y-2">
            <label className="block font-semibold text-[var(--ink)]">
              서명 행사 자격 (Acting Capacity) 선택:
            </label>
            <p className="text-[11px] text-[var(--steel)]">
              한 사람은 여러 역할을 동시에 보유할 수 있으므로, 본 승인을 집행하는
              구체적인 직무 자격을 지정해야 합니다.
            </p>

            <div className="space-y-2">
              {capacities.length === 0 ? (
                <div className="p-3 text-center text-slate-400 border border-dashed rounded-md">
                  행사 가능한 유효 결재 권한이 없습니다.
                </div>
              ) : (
                capacities.map((cap) => {
                  const isExceeded =
                    cap.maxAmount !== undefined && docAmount > cap.maxAmount;
                  const isSelected = selectedRoleId === cap.roleId;

                  return (
                    <div
                      key={cap.roleId}
                      onClick={() => {
                        if (!isExceeded) {
                          setSelectedRoleId(cap.roleId);
                          setPasskeyState("idle");
                        }
                      }}
                      className={`p-3 rounded-lg border transition select-none cursor-pointer ${
                        isSelected
                          ? "border-indigo-600 bg-indigo-50/60 dark:bg-indigo-950/40 ring-1 ring-indigo-500"
                          : isExceeded
                          ? "border-rose-200 bg-rose-50/20 opacity-60 cursor-not-allowed"
                          : "border-[var(--border)] hover:bg-[var(--muted)]"
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2">
                          <span
                            className="w-3 h-3 rounded-full shrink-0"
                            style={{ backgroundColor: cap.color }}
                          />
                          <span className="font-bold text-[var(--ink)]">
                            {cap.roleName}
                          </span>
                        </div>
                        {cap.maxAmount ? (
                          <span
                            className={`font-mono text-[11px] px-1.5 py-0.5 rounded ${
                              isExceeded
                                ? "bg-rose-100 text-rose-700 dark:bg-rose-950 dark:text-rose-300 font-bold"
                                : "bg-black/5 dark:bg-white/10 text-[var(--steel)]"
                            }`}
                          >
                            전결한도: ₩{cap.maxAmount.toLocaleString()}
                          </span>
                        ) : (
                          <span className="text-[10px] px-1.5 py-0.5 rounded bg-emerald-100 dark:bg-emerald-950/50 text-emerald-700 dark:text-emerald-300 font-medium">
                            전결 상한 없음
                          </span>
                        )}
                      </div>

                      <div className="mt-1.5 flex items-center justify-between text-[11px] text-[var(--steel)]">
                        <span>근거: {cap.justification}</span>
                        {cap.requiresPasskey && (
                          <span className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-400 font-medium">
                            <Fingerprint size={12} />
                            FIDO2 Passkey 필수
                          </span>
                        )}
                      </div>

                      {isExceeded && (
                        <div className="mt-2 text-[11px] text-rose-600 dark:text-rose-400 flex items-center gap-1 font-medium">
                          <AlertCircle size={12} />
                          <span>
                            품의 금액이 1단계 전결 상한(₩{cap.maxAmount?.toLocaleString()})을 초과하여 전결권 행사 불가
                          </span>
                        </div>
                      )}
                    </div>
                  );
                })
              )}
            </div>
          </div>

          {/* FIDO2 Biometric Ceremony if Required */}
          {requiresPasskey && !isOverLimit && (
            <div className="p-3.5 rounded-lg border border-amber-300 dark:border-amber-800 bg-amber-50/50 dark:bg-amber-950/30 space-y-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-1.5 font-bold text-amber-900 dark:text-amber-200">
                  <Fingerprint size={16} className="text-amber-600" />
                  <span>FIDO2 / Touch ID 생체 서명 인증</span>
                </div>
                {passkeyState === "verified" && (
                  <span className="inline-flex items-center gap-1 text-[11px] font-bold text-emerald-600 dark:text-emerald-400">
                    <CheckCircle2 size={13} />
                    서명 검증 완료
                  </span>
                )}
              </div>
              <p className="text-[11px] text-amber-800 dark:text-amber-300 leading-relaxed">
                전자서명법 제3조 및 K-SOX 내부통제 규정에 의거, 고위험 직무 전결 시
                기기 내장 하드웨어 보안 영역(Secure Enclave) 생체 서명이 요구됩니다.
              </p>

              {passkeyState !== "verified" ? (
                <Button
                  type="button"
                  variant="brand"
                  size="sm"
                  className="w-full mt-1"
                  loading={passkeyState === "verifying"}
                  leftIcon={<Fingerprint size={14} />}
                  onClick={handlePasskeyAuth}
                >
                  {passkeyState === "verifying"
                    ? "생체 서명 키 생성 중..."
                    : "Touch ID / Passkey 서명 실행"}
                </Button>
              ) : null}

              {passkeyError && (
                <p className="text-[11px] text-rose-600 font-semibold">{passkeyError}</p>
              )}
            </div>
          )}

          {/* Comment */}
          <div>
            <label className="block font-semibold text-[var(--ink)] mb-1">
              승인 의견 (Approval Note)
            </label>
            <textarea
              rows={2}
              value={comment}
              onChange={(e) => setComment(e.target.value)}
              placeholder="전결 사규 및 관련 증빙 검토 완료..."
              className="w-full px-3 py-2 text-xs rounded-md bg-[var(--surface)] border border-[var(--border)] text-[var(--ink)] focus:outline-hidden focus:ring-1 focus:ring-indigo-500"
            />
          </div>
        </div>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-[var(--border)] flex items-center justify-end gap-2 bg-[var(--muted)]/30">
          <Button variant="ghost" size="sm" onClick={onClose}>
            취소
          </Button>
          <Button
            variant="brand"
            size="sm"
            disabled={!activeCapacity || isOverLimit || (requiresPasskey && passkeyState !== "verified")}
            leftIcon={<ShieldCheck size={14} />}
            onClick={handleConfirm}
          >
            {activeCapacity ? `[${activeCapacity.roleName}] 자격으로 승인` : "승인 불가"}
          </Button>
        </div>
      </div>
    </div>
  );
}
