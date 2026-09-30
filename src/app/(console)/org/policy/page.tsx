"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import {
  ShieldAlert,
  Clock,
  FileCheck2,
  Calendar,
  Save,
  KeyRound,
  RotateCcw,
  CheckCircle2,
  AlertTriangle,
  Sliders,
  Users,
  Search,
  Wrench,
} from "lucide-react";
import { ConstrainedPolicyBuilder } from "@/components/gov/ConstrainedPolicyBuilder";
import { AccessExplanationInspector } from "@/components/gov/AccessExplanationInspector";
import { OperationalRecoveryCenter } from "@/components/gov/OperationalRecoveryCenter";

export default function PolicySetupPage() {
  const orgPolicy = useAppStore((s) => s.orgPolicy);
  const updateOrgPolicy = useAppStore((s) => s.updateOrgPolicy);
  const addToast = useAppStore((s) => s.addToast);

  const [policyTab, setPolicyTab] = useState<"statutory" | "builder" | "inspector" | "recovery">("statutory");

  // 로컬 폼 상태
  const [standardWeekly, setStandardWeekly] = useState(orgPolicy.attendance.standardWeeklyHours);
  const [standardDaily, setStandardDaily] = useState(orgPolicy.attendance.standardDailyHours);
  const [maxWeekly, setMaxWeekly] = useState(orgPolicy.attendance.maxWeeklyHours);
  const [warningWeekly, setWarningWeekly] = useState(orgPolicy.attendance.warningWeeklyHours);
  const [break4h, setBreak4h] = useState(orgPolicy.attendance.statutoryRestBreak4hMinutes);
  const [break8h, setBreak8h] = useState(orgPolicy.attendance.statutoryRestBreak8hMinutes);
  const [nightStart, setNightStart] = useState(orgPolicy.attendance.nightShiftStart);
  const [nightEnd, setNightEnd] = useState(orgPolicy.attendance.nightShiftEnd);
  const [overtimePreApprove, setOvertimePreApprove] = useState(orgPolicy.attendance.overtimePreApprovalRequired);

  // DoA 한도
  const [tier1Limit, setTier1Limit] = useState(orgPolicy.doaTiers[0]?.maxAmount || 5000000);
  const [tier2Limit, setTier2Limit] = useState(orgPolicy.doaTiers[1]?.maxAmount || 50000000);

  // 연차 규정
  const [accrualBasis, setAccrualBasis] = useState(orgPolicy.leave.accrualBasis);
  const [annualGrant, setAnnualGrant] = useState(orgPolicy.leave.standardAnnualGrant);
  const [promo1Months, setPromo1Months] = useState(orgPolicy.leave.statutoryPromotionRound1MonthsBefore);
  const [promo2Months, setPromo2Months] = useState(orgPolicy.leave.statutoryPromotionRound2MonthsBefore);

  const handleSavePolicy = (e: React.FormEvent) => {
    e.preventDefault();

    updateOrgPolicy({
      attendance: {
        standardWeeklyHours: Number(standardWeekly),
        standardDailyHours: Number(standardDaily),
        maxWeeklyHours: Number(maxWeekly),
        warningWeeklyHours: Number(warningWeekly),
        statutoryRestBreak4hMinutes: Number(break4h),
        statutoryRestBreak8hMinutes: Number(break8h),
        nightShiftStart: nightStart,
        nightShiftEnd: nightEnd,
        overtimePreApprovalRequired: overtimePreApprove,
      },
      doaTiers: [
        { id: "tier_1", maxAmount: Number(tier1Limit), label: `${(tier1Limit / 10000).toLocaleString()}만원 이하 (부서장 전결)`, approverRole: "부서장", requirePasskey: false },
        { id: "tier_2", maxAmount: Number(tier2Limit), label: `${(tier1Limit / 10000).toLocaleString()}만~${(tier2Limit / 10000).toLocaleString()}만원 (총괄본부장 C-Level)`, approverRole: "총괄본부장", requirePasskey: true },
        { id: "tier_3", maxAmount: 9999999999, label: `${(tier2Limit / 10000).toLocaleString()}만원 초과 (대표이사/회장단)`, approverRole: "대표이사", requirePasskey: true },
      ],
      leave: {
        accrualBasis,
        firstYearMonthlyGrant: true,
        standardAnnualGrant: Number(annualGrant),
        statutoryPromotionRound1MonthsBefore: Number(promo1Months),
        statutoryPromotionRound2MonthsBefore: Number(promo2Months),
      },
    });
  };

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "사규 버전", value: orgPolicy.version, badge: "LSA 2026 준수", badgeTone: "ok" },
          { label: "법정 소정근로", value: `주 ${orgPolicy.attendance.standardWeeklyHours}시간`, subValue: "1일 8시간 기준" },
          { label: "주52시간 게이트", value: `${orgPolicy.attendance.warningWeeklyHours}h 경고 / 52h 차단`, badgeTone: "warn" },
          { label: "최종 개정일", value: orgPolicy.updatedAt.slice(0, 10), subValue: orgPolicy.updatedBy },
        ]}
      />

      {/* 2. 사규 및 권한 정책 탭 네비게이션 */}
      <div className="flex items-center gap-1.5 border-b border-[var(--border)] pb-2 overflow-x-auto">
        <button
          type="button"
          onClick={() => setPolicyTab("statutory")}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
            policyTab === "statutory"
              ? "bg-indigo-600 text-white shadow-2xs"
              : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
          }`}
        >
          <Sliders className="w-4 h-4" />
          <span>법정 사규 및 전결 (취업규칙 · DoA · 연차)</span>
        </button>

        <button
          type="button"
          onClick={() => setPolicyTab("builder")}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
            policyTab === "builder"
              ? "bg-indigo-600 text-white shadow-2xs"
              : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
          }`}
        >
          <Users className="w-4 h-4" />
          <span>Cedar 직무 권한 배정 (Policy Builder)</span>
        </button>

        <button
          type="button"
          onClick={() => setPolicyTab("inspector")}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
            policyTab === "inspector"
              ? "bg-indigo-600 text-white shadow-2xs"
              : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
          }`}
        >
          <Search className="w-4 h-4" />
          <span>접근 권한 진단기 (Access Inspector)</span>
        </button>

        <button
          type="button"
          onClick={() => setPolicyTab("recovery")}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
            policyTab === "recovery"
              ? "bg-indigo-600 text-white shadow-2xs"
              : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
          }`}
        >
          <Wrench className="w-4 h-4" />
          <span>운영 장애 셀프 복구 (Recovery Center)</span>
        </button>
      </div>

      {policyTab === "statutory" && (
        <form onSubmit={handleSavePolicy} className="space-y-5">
        {/* 섹션 1: 근로기준법 근태 및 52시간 게이트 규정 */}
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
            <div className="flex items-center gap-2">
              <Clock className="w-5 h-5 text-amber-600" />
              <div>
                <h3 className="font-bold text-sm text-[var(--ink)]">
                  근태 관리 및 주52시간 근로시간 상한 통제 정책 (LSA §53, §54)
                </h3>
                <span className="text-[11px] text-[var(--steel)]">
                  근로기준법 및 취업규칙에 따른 소정근로시간, 법정 휴게시간, 야간근로 및 연장근로 사전승인 기준
                </span>
              </div>
            </div>
            <StatusChip label="법정 강행규정" tone="warn" size="xs" />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 text-xs">
            <div>
              <label className="font-semibold block mb-1">소정근로시간 (주 단위, 시간)</label>
              <input
                type="number"
                value={standardWeekly}
                onChange={(e) => setStandardWeekly(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">기본 40시간 (LSA §50)</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">1일 기본 소정시간 (시간)</label>
              <input
                type="number"
                value={standardDaily}
                onChange={(e) => setStandardDaily(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">기본 8시간 기준</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">주52시간 경고 임계값 (시간)</label>
              <input
                type="number"
                value={warningWeekly}
                onChange={(e) => setWarningWeekly(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono text-amber-700 font-bold"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">도달 시 관리자 알림</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">법정 한도 절대 차단 (시간)</label>
              <input
                type="number"
                disabled
                value={maxWeekly}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--muted)] font-mono text-red-700 font-bold"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">법정 상한 52시간 절대 초과 금지</span>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 text-xs pt-2 border-t border-[var(--border)]/60">
            <div>
              <label className="font-semibold block mb-1">4시간 이상 근무 시 휴게시간 (분)</label>
              <input
                type="number"
                value={break4h}
                onChange={(e) => setBreak4h(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">법정 30분 이상 (§54)</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">8시간 이상 근무 시 휴게시간 (분)</label>
              <input
                type="number"
                value={break8h}
                onChange={(e) => setBreak8h(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">법정 60분 이상 (§54)</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">야간근로 인정 시간대 (시작/종료)</label>
              <div className="flex items-center gap-1">
                <input
                  type="text"
                  value={nightStart}
                  onChange={(e) => setNightStart(e.target.value)}
                  className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                />
                <span className="text-[var(--steel)]">~</span>
                <input
                  type="text"
                  value={nightEnd}
                  onChange={(e) => setNightEnd(e.target.value)}
                  className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                />
              </div>
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">50% 가산수당 지급</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">연장근로 사전 승인제</label>
              <div className="flex items-center gap-2 h-8">
                <input
                  type="checkbox"
                  id="otPre"
                  checked={overtimePreApprove}
                  onChange={(e) => setOvertimePreApprove(e.target.checked)}
                  className="rounded text-[var(--brand)] focus:ring-0"
                />
                <label htmlFor="otPre" className="text-xs text-[var(--ink)] cursor-pointer">
                  전자결재 사전승인 없는 연장 불인정
                </label>
              </div>
            </div>
          </div>
        </div>

        {/* 섹션 2: 전결 규정 (Delegation of Authority - DoA) 및 Passkey 정책 */}
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
            <div className="flex items-center gap-2">
              <FileCheck2 className="w-5 h-5 text-indigo-600" />
              <div>
                <h3 className="font-bold text-sm text-[var(--ink)]">
                  전자결재 및 지출결의 전결 규정 (DoA Thresholds)
                </h3>
                <span className="text-[11px] text-[var(--steel)]">
                  품의 금액 규모에 따른 승인 권한 단계 및 FIDO2 Passkey 생체인증 강제 여부
                </span>
              </div>
            </div>
            <StatusChip label="내부회계관리제도" tone="info" size="xs" />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs">
            <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-3.5 space-y-2">
              <div className="font-bold text-[var(--ink)] flex items-center justify-between">
                <span>1단계: 팀장 / 부서장 전결</span>
                <StatusChip label="전결 가능" size="xs" />
              </div>
              <div>
                <label className="font-semibold block mb-1 text-[11px]">최대 한도 금액 (원)</label>
                <input
                  type="number"
                  value={tier1Limit}
                  onChange={(e) => setTier1Limit(Number(e.target.value))}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono font-bold"
                />
              </div>
              <p className="text-[11px] text-[var(--steel)]">500만원 이하 일반 운영비, 소모품, 일상 정비 비용 전결</p>
            </div>

            <div className="rounded-lg border border-indigo-200 bg-indigo-50/40 p-3.5 space-y-2">
              <div className="font-bold text-indigo-950 flex items-center justify-between">
                <span>2단계: 총괄본부장 (C-Level)</span>
                <span className="px-1.5 py-0.5 rounded text-[10px] font-mono bg-indigo-100 text-indigo-800 font-bold">
                  Passkey 권고
                </span>
              </div>
              <div>
                <label className="font-semibold block mb-1 text-[11px]">최대 한도 금액 (원)</label>
                <input
                  type="number"
                  value={tier2Limit}
                  onChange={(e) => setTier2Limit(Number(e.target.value))}
                  className="w-full h-8 px-2.5 rounded border border-indigo-200 bg-[var(--surface)] font-mono font-bold"
                />
              </div>
              <p className="text-[11px] text-indigo-900">5,000만원 이하 장비 부품, 계약 변경 및 외주 도급비 인가</p>
            </div>

            <div className="rounded-lg border border-purple-200 bg-purple-50/40 p-3.5 space-y-2">
              <div className="font-bold text-purple-950 flex items-center justify-between">
                <span>3단계: 대표이사 및 회장단</span>
                <span className="px-1.5 py-0.5 rounded text-[10px] font-mono bg-purple-100 text-purple-800 font-bold flex items-center gap-1">
                  <KeyRound className="w-3 h-3 text-purple-700" />
                  <span>Passkey 필수</span>
                </span>
              </div>
              <div>
                <label className="font-semibold block mb-1 text-[11px]">초과 금액</label>
                <div className="h-8 px-2.5 rounded border border-purple-200 bg-[var(--surface)] font-mono font-bold flex items-center text-purple-900">
                  {tier2Limit.toLocaleString()}원 초과 전건
                </div>
              </div>
              <p className="text-[11px] text-purple-900">신규 도급 계약, 고액 자산 취득 및 이사회 부의 안건 최종 승인</p>
            </div>
          </div>
        </div>

        {/* 섹션 3: 근로기준법 제61조 연차유급휴가 및 사용촉진 정책 */}
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
            <div className="flex items-center gap-2">
              <Calendar className="w-5 h-5 text-emerald-600" />
              <div>
                <h3 className="font-bold text-sm text-[var(--ink)]">
                  연차유급휴가 발생 및 사용촉진 제도 정책 (LSA §60, §61)
                </h3>
                <span className="text-[11px] text-[var(--steel)]">
                  연차 산정 기준일, 1년 미만 월차 부여, 법정 1차 및 2차 사용촉진 일정 통제
                </span>
              </div>
            </div>
            <StatusChip label="금전보상면책 통제" tone="ok" size="xs" />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 text-xs">
            <div>
              <label className="font-semibold block mb-1">연차 부여 산정 기준</label>
              <select
                value={accrualBasis}
                onChange={(e) => setAccrualBasis(e.target.value as any)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
              >
                <option value="fiscal_year">회계연도 기준 (매년 1월 1일 일괄)</option>
                <option value="hire_date">입사일 기준 (개인별 입사기념일)</option>
              </select>
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">전사 통합 관리는 회계연도 권장</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">기본 발생 연차 (1년 이상 재직)</label>
              <input
                type="number"
                value={annualGrant}
                onChange={(e) => setAnnualGrant(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono font-bold"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">법정 15일 기본 (최대 25일)</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">1차 촉진 통지 시점 (만료 N개월 전)</label>
              <input
                type="number"
                value={promo1Months}
                onChange={(e) => setPromo1Months(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">법정 기간: 만료 6개월 전 10일간</span>
            </div>

            <div>
              <label className="font-semibold block mb-1">2차 촉진 시점 (만료 N개월 전)</label>
              <input
                type="number"
                value={promo2Months}
                onChange={(e) => setPromo2Months(Number(e.target.value))}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
              />
              <span className="text-[10px] text-[var(--faint)] block mt-0.5">법정 기간: 만료 2개월 전 시기지정</span>
            </div>
          </div>
        </div>

        {/* 저장 액션 바 */}
        <div className="flex items-center justify-between p-4 rounded-lg border border-[var(--border)] bg-[var(--canvas)]">
          <div className="flex items-center gap-2 text-xs text-[var(--steel)]">
            <ShieldAlert className="w-4 h-4 text-amber-600" />
            <span>정책 개정 시 변경 내역이 WORM 감사 체인에 영구 기록되며 전 부서에 즉시 반영됩니다.</span>
          </div>
          <Button size="sm" variant="brand" leftIcon={<Save className="w-4 h-4" />} type="submit">
            정책 개정 확정 및 전사 적용
          </Button>
        </div>
      </form>
      )}

      {policyTab === "builder" && <ConstrainedPolicyBuilder />}
      {policyTab === "inspector" && <AccessExplanationInspector />}
      {policyTab === "recovery" && <OperationalRecoveryCenter />}
    </div>
  );
}
