"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { OperationalIncident } from "@/lib/types";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import { ReviewerHandoffModal } from "./ReviewerHandoffModal";
import {
  Wrench,
  AlertTriangle,
  CheckCircle2,
  KeyRound,
  FileSpreadsheet,
  RotateCcw,
  ShieldCheck,
  Building,
  Clock,
  ArrowRight,
  X,
  Radio,
  Send,
} from "lucide-react";

export function OperationalRecoveryCenter() {
  const incidents = useAppStore((s) => s.operationalIncidents);
  const resolveIncident = useAppStore((s) => s.resolveOperationalIncident);
  const bankBatch = useAppStore((s) => s.bankTransferBatch);
  const addToast = useAppStore((s) => s.addToast);

  const [activeIncident, setActiveIncident] = useState<OperationalIncident | null>(null);
  const [handoffDocCode, setHandoffDocCode] = useState<string | null>(null);

  // Recovery wizard state
  const [certStep, setCertStep] = useState<"ready" | "pinging" | "done">("ready");
  const [importFixed, setImportFixed] = useState(false);
  const [batchResumed, setBatchResumed] = useState(false);

  const openRecoveryModal = (inc: OperationalIncident) => {
    if (inc.category === "approval_deadlock") {
      setHandoffDocCode((inc.metadata?.stalledDocCode as string) || "AP-3121");
      return;
    }
    setActiveIncident(inc);
    setCertStep("ready");
    setImportFixed(false);
    setBatchResumed(false);
  };

  const handleResolveCertIncident = () => {
    setCertStep("pinging");
    setTimeout(() => {
      setCertStep("done");
      if (activeIncident) {
        resolveIncident(
          activeIncident.id,
          "KFTC 신규 전자인증서 등록 및 은행 호스트 핑 테스트 성공 (100% 정상)"
        );
        setTimeout(() => setActiveIncident(null), 1000);
      }
    }, 1200);
  };

  const handleResolveImportIncident = () => {
    if (!activeIncident) return;
    resolveIncident(
      activeIncident.id,
      "스테이징 테이블 내 비표준 주민번호 및 누락 은행코드 3건 인라인 정제 완료"
    );
    setActiveIncident(null);
  };

  const handleResolveBatchIncident = () => {
    if (!activeIncident) return;
    resolveIncident(
      activeIncident.id,
      "멱등성 키(IDEMP-KFTC-202607-ACME) 기반 미송신 13건 정상 재개 및 이체 완결"
    );
    setActiveIncident(null);
  };

  return (
    <div className="space-y-6">
      {/* 상단 통계 요약 */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-3">
        <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
          <div className="flex items-center gap-2">
            <Wrench className="w-5 h-5 text-amber-600" />
            <div>
              <h3 className="font-bold text-sm text-[var(--ink)]">
                운영 장애 셀프 복구 센터 (Operational Self-Service Recovery)
              </h3>
              <span className="text-[11px] text-[var(--steel)]">
                개발자의 DB 쿼리나 긴급 배포 없이, 비즈니스 운영자가 즉시 해결할 수 있는 자체 복구 워크플로우를 제공합니다.
              </span>
            </div>
          </div>
          <StatusChip
            label={`${incidents.filter((i) => i.status === "open").length}건 조치 대기`}
            tone={incidents.some((i) => i.status === "open") ? "warn" : "ok"}
            size="xs"
          />
        </div>

        {/* 4대 장애 유형 인시던트 카드 그리드 */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {incidents.map((inc) => {
            const isResolved = inc.status === "resolved";
            return (
              <div
                key={inc.id}
                className={`p-4 rounded-lg border text-xs space-y-3 transition ${
                  isResolved
                    ? "border-[var(--border)] bg-[var(--canvas)] opacity-75"
                    : inc.severity === "critical"
                    ? "border-red-300 bg-red-50/30"
                    : "border-amber-300 bg-amber-50/30"
                }`}
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="flex items-center gap-2">
                    {isResolved ? (
                      <CheckCircle2 className="w-4 h-4 text-emerald-600 shrink-0" />
                    ) : inc.severity === "critical" ? (
                      <AlertTriangle className="w-4 h-4 text-red-600 shrink-0" />
                    ) : (
                      <Clock className="w-4 h-4 text-amber-600 shrink-0" />
                    )}
                    <span className="font-bold text-xs text-[var(--ink)] line-clamp-1">
                      {inc.title}
                    </span>
                  </div>
                  <StatusChip
                    label={isResolved ? "해결됨" : inc.severity === "critical" ? "긴급" : "경고"}
                    tone={isResolved ? "ok" : inc.severity === "critical" ? "danger" : "warn"}
                    size="xs"
                  />
                </div>

                <p className="text-[11px] text-[var(--steel)] leading-relaxed">
                  {inc.description}
                </p>

                <div className="flex items-center justify-between pt-2 border-t border-[var(--border)]/60 text-[10px] text-[var(--faint)]">
                  <span className="flex items-center gap-1">
                    <Building className="w-3 h-3" />
                    <span>{inc.affectedEntity}</span>
                  </span>
                  <span className="font-mono">{inc.createdAt.slice(0, 16).replace("T", " ")}</span>
                </div>

                {/* 복구 실행 버튼 또는 완료 메시지 */}
                <div className="pt-1">
                  {isResolved ? (
                    <div className="p-2 rounded bg-emerald-50 border border-emerald-200 text-emerald-800 text-[11px] space-y-0.5">
                      <div className="font-bold flex items-center gap-1">
                        <CheckCircle2 className="w-3.5 h-3.5" />
                        <span>복구 완료 ({inc.resolvedAt})</span>
                      </div>
                      <p className="text-[10px]">{inc.resolutionNote}</p>
                    </div>
                  ) : (
                    <Button
                      size="xs"
                      variant={inc.severity === "critical" ? "danger" : "brand"}
                      className="w-full"
                      onClick={() => openRecoveryModal(inc)}
                      leftIcon={<Wrench className="w-3.5 h-3.5" />}
                    >
                      {inc.recoveryActionLabel}
                    </Button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* 복구 상세 모달 */}
      {activeIncident && (
        <div className="fixed inset-0 bg-black/50 backdrop-blur-xs flex items-center justify-center z-50 p-4">
          <div className="bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-2xl max-w-lg w-full p-6 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-start justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Wrench className="w-5 h-5 text-indigo-600" />
                <div>
                  <h3 className="font-bold text-sm text-[var(--ink)]">
                    {activeIncident.title}
                  </h3>
                  <span className="text-[11px] text-[var(--steel)]">
                    식별번호: <strong className="font-mono">{activeIncident.id}</strong> · 관할: {activeIncident.affectedEntity}
                  </span>
                </div>
              </div>
              <button
                onClick={() => setActiveIncident(null)}
                className="p-1 rounded text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
              >
                <X className="w-5 h-5" />
              </button>
            </div>

            {/* A. 펌뱅킹 전자인증서 갱신 위저드 */}
            {activeIncident.category === "banking_cert" && (
              <div className="space-y-3 text-xs">
                <div className="p-3 rounded-lg border border-[var(--border)] bg-[var(--canvas)] space-y-1.5">
                  <div className="font-bold text-[var(--ink)]">신한은행 CMS 클라이언트 인증서 상태</div>
                  <div className="grid grid-cols-2 gap-2 text-[11px] text-[var(--steel)]">
                    <div>시리얼 번호: <strong className="font-mono text-[var(--ink)]">7B98-E412-00F1</strong></div>
                    <div>만료 예정: <strong className="text-red-600 font-bold">2026-09-24 (D-3)</strong></div>
                    <div>출금 모계좌: <strong className="font-mono text-[var(--ink)]">110-452-987654</strong></div>
                    <div>검증 상태: <strong className="text-amber-600">갱신 대기</strong></div>
                  </div>
                </div>

                <p className="text-[11px] text-[var(--steel)] leading-relaxed">
                  금융결제원(KFTC) 공인인증 서버와 상호 TLS 키 교환(Mutual TLS Handshake)을 수행하여 신규 1년짜리 전자서명 인증서로 무중단 갱신합니다.
                </p>

                {certStep === "pinging" && (
                  <div className="p-3 rounded-lg border border-indigo-200 bg-indigo-50/50 flex items-center gap-3 text-indigo-950 font-bold">
                    <Radio className="w-5 h-5 text-indigo-600 animate-pulse" />
                    <span>금융결제원 펌뱅킹 호스트와 핑 테스트 진행 중...</span>
                  </div>
                )}

                {certStep === "done" && (
                  <div className="p-3 rounded-lg border border-emerald-200 bg-emerald-50 flex items-center gap-3 text-emerald-900 font-bold">
                    <CheckCircle2 className="w-5 h-5 text-emerald-600" />
                    <span>인증서 갱신 및 핑 테스트 100% 성공! (검증 게이트 개방)</span>
                  </div>
                )}

                <div className="flex items-center justify-end gap-2 pt-2 border-t border-[var(--border)]">
                  <Button size="sm" variant="ghost" onClick={() => setActiveIncident(null)}>
                    닫기
                  </Button>
                  <Button
                    size="sm"
                    variant="brand"
                    disabled={certStep !== "ready"}
                    onClick={handleResolveCertIncident}
                    leftIcon={<KeyRound className="w-4 h-4" />}
                  >
                    인증서 자체 갱신 및 키 교환 실행
                  </Button>
                </div>
              </div>
            )}

            {/* B. 대량 업로드 오류 정제 위저드 */}
            {activeIncident.category === "import_validation" && (
              <div className="space-y-3 text-xs">
                <div className="p-3 rounded-lg border border-red-200 bg-red-50/50 space-y-1">
                  <div className="font-bold text-red-950">검증 차단된 레코드 (3건)</div>
                  <div className="space-y-1 text-[11px] text-red-900">
                    <div>1. 박정비: 은행코드 999 미등록 → <strong>신한(088)</strong>으로 교정됨</div>
                    <div>2. 최운송: 주민번호 13자리 비표준 → <strong>표준 식별자 형식</strong>으로 정규화됨</div>
                    <div>3. 강하역: 기본급 필드 누락 → <strong>계약서(C-2026-003)</strong> 기반 280만원 자동 채움</div>
                  </div>
                </div>

                <label className="flex items-center gap-2 cursor-pointer pt-1">
                  <input
                    type="checkbox"
                    checked={importFixed}
                    onChange={(e) => setImportFixed(e.target.checked)}
                    className="rounded text-[var(--brand)]"
                  />
                  <span className="font-bold text-[var(--ink)]">
                    오류 3건에 대한 교정값을 확인하였으며, 급여 계산 게이트에 일괄 적용합니다.
                  </span>
                </label>

                <div className="flex items-center justify-end gap-2 pt-2 border-t border-[var(--border)]">
                  <Button size="sm" variant="ghost" onClick={() => setActiveIncident(null)}>
                    취소
                  </Button>
                  <Button
                    size="sm"
                    variant="brand"
                    disabled={!importFixed}
                    onClick={handleResolveImportIncident}
                    leftIcon={<CheckCircle2 className="w-4 h-4" />}
                  >
                    데이터 정제 확정 및 게이트 재검증
                  </Button>
                </div>
              </div>
            )}

            {/* C. 펌뱅킹 네트워크 타임아웃 세션 중단 복구 */}
            {activeIncident.category === "batch_interrupted" && (
              <div className="space-y-3 text-xs">
                <div className="p-3 rounded-lg border border-amber-200 bg-amber-50/50 space-y-2">
                  <div className="font-bold text-amber-950">배치 송신 세션 중단 현황</div>
                  <div className="grid grid-cols-2 gap-2 text-[11px] text-amber-900">
                    <div>기 송신 완료: <strong className="font-mono text-emerald-700">32건 (2억 1,200만원)</strong></div>
                    <div>미송신 중단: <strong className="font-mono text-red-600 font-bold">13건 (7,340만원)</strong></div>
                    <div>멱등성 키: <strong className="font-mono">IDEMP-KFTC-202607-ACME</strong></div>
                    <div>중복 방지 락: <strong className="text-emerald-700">활성 (Double-Pay 방지)</strong></div>
                  </div>
                </div>

                <p className="text-[11px] text-[var(--steel)] leading-relaxed">
                  은행 호스트와 잔여 패킷을 대조하여, 이미 지급 완료된 32건은 스킵하고 미송신된 13건만 멱등성 보장 하에 순차적으로 이어서 송신합니다.
                </p>

                <label className="flex items-center gap-2 cursor-pointer pt-1">
                  <input
                    type="checkbox"
                    checked={batchResumed}
                    onChange={(e) => setBatchResumed(e.target.checked)}
                    className="rounded text-[var(--brand)]"
                  />
                  <span className="font-bold text-[var(--ink)]">
                    중복 송신 방지 대조표를 확인하였으며 잔여 13건 재개를 승인합니다.
                  </span>
                </label>

                <div className="flex items-center justify-end gap-2 pt-2 border-t border-[var(--border)]">
                  <Button size="sm" variant="ghost" onClick={() => setActiveIncident(null)}>
                    취소
                  </Button>
                  <Button
                    size="sm"
                    variant="brand"
                    disabled={!batchResumed}
                    onClick={handleResolveBatchIncident}
                    leftIcon={<Send className="w-4 h-4" />}
                  >
                    미송신 13건 이어서 송신 완료
                  </Button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {/* 대결 모달 연계 */}
      {handoffDocCode && (
        <ReviewerHandoffModal
          isOpen={true}
          onClose={() => setHandoffDocCode(null)}
          preselectedDocCode={handoffDocCode}
        />
      )}
    </div>
  );
}
