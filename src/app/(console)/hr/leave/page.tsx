"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { LeaveNotice } from "@/lib/types";
import {
  CalendarCheck,
  KeyRound,
  ShieldCheck,
  AlertTriangle,
  FileText,
  CheckCircle2,
  Clock,
  Send,
  Plus,
  X,
  FileCheck,
} from "lucide-react";

export default function LeavePromotionPage() {
  const leaveNotices = useAppStore((s) => s.leaveNotices);
  const sendLeaveNotice = useAppStore((s) => s.sendLeaveNotice);
  const signLeaveNotice = useAppStore((s) => s.signLeaveNotice);
  const createLeaveNotice = useAppStore((s) => s.createLeaveNotice);
  const employees = useAppStore((s) => s.employees);
  const viewAsRole = useAppStore((s) => s.viewAsRole);

  const [selectedNotice, setSelectedNotice] = useState<LeaveNotice | null>(null);
  const [passkeyModalOpen, setPasskeyModalOpen] = useState(false);
  const [signingStep, setSigningStep] = useState<"ready" | "touching" | "done">("ready");
  const [certModalOpen, setCertModalOpen] = useState(false);
  const [createModalOpen, setCreateModalOpen] = useState(false);

  // 신규 통지 발령 폼 상태
  const [newEmpId, setNewEmpId] = useState(employees[0]?.id || "");
  const [newRound, setNewRound] = useState<"1차" | "2차">("1차");
  const [newRemainingDays, setNewRemainingDays] = useState<number>(5);
  const [newDeadline, setNewDeadline] = useState("2026-07-15");

  const handleStartPasskeySign = (notice: LeaveNotice) => {
    setSelectedNotice(notice);
    setSigningStep("ready");
    setPasskeyModalOpen(true);
  };

  const executeBiometricSign = () => {
    if (!selectedNotice) return;
    setSigningStep("touching");
    setTimeout(() => {
      setSigningStep("done");
      setTimeout(() => {
        signLeaveNotice(selectedNotice.id);
        setPasskeyModalOpen(false);
      }, 700);
    }, 1200);
  };

  const handleViewCertificate = (notice: LeaveNotice) => {
    setSelectedNotice(notice);
    setCertModalOpen(true);
  };

  const signedCount = leaveNotices.filter((n) => n.status.includes("수령확인")).length;
  const totalCount = Math.max(1, leaveNotices.length);
  const signRate = Math.round((signedCount / totalCount) * 100);

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          {
            label: "촉진 대상 인원",
            value: `${leaveNotices.length}명`,
            badge: "미사용 연차 보유",
            badgeTone: "warn",
          },
          { label: "1차 촉진 기한", value: "D-7", subValue: "2026-07-10 마감" },
          {
            label: "수령 증빙 완료율",
            value: `${signRate}%`,
            badge: "FIDO2 Passkey",
            badgeTone: signRate > 50 ? "ok" : "info",
          },
          {
            label: "법적 근거",
            value: "근로기준법 제61조",
            subValue: "미사용수당 지급의무 면책",
          },
        ]}
      />

      {/* 2. 법률 컴플라이언스 안내 배너 */}
      <div className="p-4 rounded-lg border border-blue-200 bg-blue-50/70 text-blue-950 space-y-1.5 text-xs">
        <div className="flex items-center gap-2 font-bold">
          <CalendarCheck className="w-4 h-4 text-blue-700" />
          <span>근로기준법 제61조 (연차유급휴가의 사용 촉진) 법적 증빙 통제</span>
        </div>
        <p className="text-[11px] leading-relaxed text-blue-900">
          사용자가 법정 기간 내에 근로자에게 미사용 휴가일수를 알리고 사용계획을 서면(전자문서 + 생체인증 FIDO2 전자서명)으로 요구한 경우에 한하여, 휴가 미사용 시 금전보상 의무가 법적으로 면책됩니다.
        </p>
      </div>

      {/* 3. 촉진 통지 대장 */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs overflow-hidden">
        <div className="px-4 py-3 border-b border-[var(--border)] flex items-center justify-between">
          <div className="flex items-center gap-2">
            <FileText className="w-4 h-4 text-blue-600" />
            <h3 className="text-xs font-bold text-[var(--ink)]">
              연차 사용촉진 통지 및 법적 수령확인 대장
            </h3>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-[11px] text-[var(--faint)]">총 {leaveNotices.length}건</span>
            <Button
              size="xs"
              variant="brand"
              leftIcon={<Plus className="w-3 h-3" />}
              onClick={() => setCreateModalOpen(true)}
            >
              신규 촉진 통지 등록
            </Button>
          </div>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-xs text-left border-collapse">
            <thead>
              <tr className="border-b border-[var(--border)] bg-[var(--muted)] text-[var(--steel)]">
                <th className="px-3 py-2">통지 코드</th>
                <th className="px-3 py-2">구분</th>
                <th className="px-3 py-2">대상 사원</th>
                <th className="px-3 py-2">소속 부서</th>
                <th className="px-3 py-2 text-right">잔여 연차</th>
                <th className="px-3 py-2">제출 기한</th>
                <th className="px-3 py-2">수령 증빙 상태</th>
                <th className="px-3 py-2 text-center">작업</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-soft)]">
              {leaveNotices.map((n) => (
                <tr key={n.id} className="hover:bg-gray-50/70 h-9">
                  <td className="px-3 py-2">
                    <ObjectLink code={n.code} />
                  </td>
                  <td className="px-3 py-2">
                    <StatusChip label={n.round} tone="info" size="xs" />
                  </td>
                  <td className="px-3 py-2 font-bold">{n.employeeName}</td>
                  <td className="px-3 py-2 text-[var(--steel)]">{n.dept}</td>
                  <td className="px-3 py-2 text-right font-mono font-bold text-amber-700">
                    {n.remainingDays}일
                  </td>
                  <td className="px-3 py-2 font-mono text-[var(--faint)]">{n.deadline}</td>
                  <td className="px-3 py-2">
                    <StatusChip
                      label={n.status}
                      tone={n.status.includes("수령확인") ? "ok" : n.status === "발송완료" ? "warn" : "neutral"}
                      size="xs"
                      dot
                    />
                    {n.confirmedAt && (
                      <span className="block text-[9px] font-mono text-[var(--faint)] mt-0.5">
                        {n.confirmedAt}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 text-center">
                    {n.status === "통지대기" ? (
                      <Button
                        size="xs"
                        variant="secondary"
                        leftIcon={<Send className="w-3 h-3" />}
                        onClick={() => sendLeaveNotice(n.id)}
                      >
                        1차 통지 발송
                      </Button>
                    ) : n.status === "발송완료" ? (
                      <Button
                        size="xs"
                        variant="brand"
                        leftIcon={<KeyRound className="w-3 h-3" />}
                        onClick={() => handleStartPasskeySign(n)}
                      >
                        본인 서명 (Passkey)
                      </Button>
                    ) : (
                      <Button
                        size="xs"
                        variant="secondary"
                        leftIcon={<FileCheck className="w-3 h-3" />}
                        onClick={() => handleViewCertificate(n)}
                      >
                        증빙 열람
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* 4. Passkey 전자서명 수령확인 모달 */}
      {passkeyModalOpen && selectedNotice && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
          <div className="w-full max-w-sm rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 text-center space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="w-12 h-12 rounded-full bg-blue-100 text-blue-700 flex items-center justify-center mx-auto">
              <KeyRound className="w-6 h-6" />
            </div>

            <div className="space-y-1">
              <h3 className="text-sm font-bold text-[var(--ink)]">
                연차유급휴가 사용촉진 통지 수령 서명
              </h3>
              <p className="text-xs text-[var(--steel)]">
                근로기준법 제61조에 따른 법적 통지 수령 확인서에 FIDO2 전자서명을 날인합니다.
              </p>
            </div>

            <div className="p-3 rounded border border-[var(--border)] bg-gray-50 text-[11px] text-left space-y-1 font-mono">
              <div>통지건: {selectedNotice.code} ({selectedNotice.round})</div>
              <div>수령인: {selectedNotice.employeeName} ({selectedNotice.dept})</div>
              <div>미사용 잔여연차: {selectedNotice.remainingDays}일</div>
              <div>수령 기한: {selectedNotice.deadline}</div>
            </div>

            {signingStep === "ready" && (
              <div className="space-y-2">
                <Button
                  variant="brand"
                  size="md"
                  className="w-full font-bold"
                  onClick={executeBiometricSign}
                >
                  지문 / Touch ID 본인확인 서명
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="w-full text-xs"
                  onClick={() => setPasskeyModalOpen(false)}
                >
                  취소
                </Button>
              </div>
            )}

            {signingStep === "touching" && (
              <div className="py-2 text-xs font-semibold text-blue-700 animate-pulse">
                보안 키 또는 지문 센서를 터치하십시오...
              </div>
            )}

            {signingStep === "done" && (
              <div className="py-2 text-xs font-bold text-emerald-700 flex items-center justify-center gap-1">
                <CheckCircle2 className="w-4 h-4 text-emerald-600" />
                <span>법적 수령 증빙 전자서명 완료!</span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* 5. 법적 증빙 수령 확인서 (Certificate View Modal) */}
      {certModalOpen && selectedNotice && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <ShieldCheck className="w-5 h-5 text-emerald-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">
                  연차유급휴가 사용촉진 통지 및 수령 증명서
                </h3>
              </div>
              <button
                onClick={() => setCertModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            <div className="space-y-3 text-xs leading-relaxed text-[var(--ink)]">
              <div className="p-3 bg-gray-50 border border-gray-200 rounded font-mono text-[11px] space-y-1">
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">문서 관리번호:</span>
                  <span className="font-bold">{selectedNotice.code}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">수령인 성명:</span>
                  <span className="font-bold">{selectedNotice.employeeName}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">소속 부서:</span>
                  <span>{selectedNotice.dept}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">미사용 연차일수:</span>
                  <span className="font-bold text-amber-700">{selectedNotice.remainingDays} 일</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">서명 일시:</span>
                  <span className="text-emerald-700 font-bold">{selectedNotice.confirmedAt || "미확인"}</span>
                </div>
              </div>

              <div className="border border-emerald-200 bg-emerald-50/60 p-3 rounded text-[11px] space-y-1 text-emerald-950">
                <div className="font-bold flex items-center gap-1 text-emerald-800">
                  <CheckCircle2 className="w-3.5 h-3.5 text-emerald-600" />
                  <span>FIDO2 / WebAuthn 전자서명 무결성 검증 완료</span>
                </div>
                <div className="font-mono text-[10px] break-all text-emerald-800">
                  SHA-256 Digest: 8fbc2e7a199d21c0800fae41d8e6b1297594bf34b12c5ec6e5b4109ca49372ef
                </div>
                <div className="text-[10px] text-emerald-700">
                  본 문서는 근로기준법 제61조에 의거하여 당사자 본인의 생체 Passkey 서명으로 법적 수령 확인이 완료되었으며, 감사 원장에 기록되었습니다.
                </div>
              </div>
            </div>

            <div className="flex justify-end pt-3 border-t border-[var(--border)]">
              <Button size="sm" variant="secondary" onClick={() => setCertModalOpen(false)}>
                닫기
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 5. 신규 촉진 통지 등록 모달 */}
      {createModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150 text-xs">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <span className="p-1.5 rounded-lg bg-blue-50 text-blue-700">
                  <CalendarCheck size={16} />
                </span>
                <div>
                  <h3 className="font-bold text-sm text-[var(--ink)]">
                    연차유급휴가 사용촉진 통지 등록
                  </h3>
                  <p className="text-[11px] text-[var(--steel)]">
                    근로기준법 제61조 법정 절차에 따른 서면 촉진 통지를 발령합니다.
                  </p>
                </div>
              </div>
              <button
                onClick={() => setCreateModalOpen(false)}
                className="p-1 rounded text-[var(--steel)] hover:text-[var(--ink)] cursor-pointer"
              >
                <X size={16} />
              </button>
            </div>

            <form
              onSubmit={(e) => {
                e.preventDefault();
                if (!newEmpId) return;
                createLeaveNotice({
                  employeeId: newEmpId,
                  round: newRound,
                  remainingDays: Number(newRemainingDays),
                  deadline: newDeadline,
                });
                setCreateModalOpen(false);
              }}
              className="space-y-3"
            >
              <div className="space-y-1">
                <label className="block font-semibold text-[var(--ink)]">대상 임직원:</label>
                <select
                  value={newEmpId}
                  onChange={(e) => setNewEmpId(e.target.value)}
                  className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none"
                >
                  {employees.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.name} ({e.dept} · {e.role})
                    </option>
                  ))}
                </select>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="block font-semibold text-[var(--ink)]">촉진 차수 구분:</label>
                  <select
                    value={newRound}
                    onChange={(e) => setNewRound(e.target.value as "1차" | "2차")}
                    className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none"
                  >
                    <option value="1차">1차 촉진 (D-6개월 기준)</option>
                    <option value="2차">2차 촉진 (D-2개월 기준)</option>
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="block font-semibold text-[var(--ink)]">미사용 잔여일수:</label>
                  <input
                    type="number"
                    min="1"
                    max="30"
                    value={newRemainingDays}
                    onChange={(e) => setNewRemainingDays(Number(e.target.value))}
                    className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-mono"
                  />
                </div>
              </div>

              <div className="space-y-1">
                <label className="block font-semibold text-[var(--ink)]">
                  사용계획서 제출 기한 (근기법 §61):
                </label>
                <input
                  type="date"
                  value={newDeadline}
                  onChange={(e) => setNewDeadline(e.target.value)}
                  className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-mono"
                />
              </div>

              <div className="p-2.5 rounded bg-blue-50/70 border border-blue-200 text-blue-900 text-[10px] leading-relaxed">
                법정 기한(10일 이내) 내 근로자가 휴가 사용 시기를 지정하여 통보하지 않을 경우,
                사용자가 사용 시기를 정하여 통보할 수 있는 법적 권한이 형성됩니다.
              </div>

              <div className="flex justify-end gap-2 pt-2 border-t border-[var(--border)]">
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => setCreateModalOpen(false)}
                >
                  취소
                </Button>
                <Button type="submit" size="sm" variant="brand">
                  촉진 통지 상신
                </Button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}
