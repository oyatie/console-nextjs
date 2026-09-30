"use client";

import React, { useState } from "react";
import Link from "next/link";
import { useAppStore } from "@/lib/store";
import { DataTable, Column } from "@/components/ui/DataTable";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Button } from "@/components/ui/Button";
import { StatBar } from "@/components/ui/StatBar";
import { AttendanceRecord } from "@/lib/types";
import { calculateWorkHours, evaluate52HourCompliance } from "@/lib/attendance-calculator";
import { ModuleScopeFilter, ModuleScopeState } from "@/components/gov/ModuleScopeFilter";
import {
  Clock,
  AlertTriangle,
  CheckCircle2,
  Calendar,
  Filter,
  ArrowRight,
  ShieldAlert,
} from "lucide-react";

export default function AttendancePage() {
  const attendance = useAppStore((s) => s.attendance);
  const payrollRun = useAppStore((s) => s.payrollRun);
  const updateAttendancePunch = useAppStore((s) => s.updateAttendancePunch);
  const adjudicateAttendanceException = useAppStore((s) => s.adjudicateAttendanceException);

  // 모달 상태
  const [selectedRecord, setSelectedRecord] = useState<AttendanceRecord | null>(null);
  const [adjudicationModalOpen, setAdjudicationModalOpen] = useState(false);

  // 편집 폼 필드
  const [editIn, setEditIn] = useState("");
  const [editOut, setEditOut] = useState("");
  const [editBreak, setEditBreak] = useState<number>(60);
  const [resolutionType, setResolutionType] = useState<"approved" | "corrected" | "rejected">("approved");
  const [justificationNote, setJustificationNote] = useState("");

  // 모듈 관할 인가 범위 필터 상태
  const [scopeFilter, setScopeFilter] = useState<ModuleScopeState>({
    entityId: "all",
    siteId: "all",
    deptId: "all",
  });

  const filteredAttendance = React.useMemo(() => {
    return attendance.filter((a) => {
      if (scopeFilter.entityId !== "all" && a.entity !== scopeFilter.entityId) return false;
      if (scopeFilter.siteId !== "all" && a.site !== scopeFilter.siteId) return false;
      return true;
    });
  }, [attendance, scopeFilter]);

  const unresolvedCount = filteredAttendance.filter((a) => a.exceptionCode).length;

  const handleOpenAdjudication = (record: AttendanceRecord) => {
    setSelectedRecord(record);
    setEditIn(record.clockIn || "09:00");
    setEditOut(record.clockOut || "18:00");
    setEditBreak(record.breakMinutes || 60);
    setJustificationNote(record.exceptionCode ? "선적 지연에 따른 도급사 특별연장 사전승인(AP-3121) 연계 확인" : "");
    setAdjudicationModalOpen(true);
  };

  const executeAdjudication = () => {
    if (!selectedRecord) return;

    if (resolutionType === "corrected") {
      // 시간 직접 보정 적용
      updateAttendancePunch(selectedRecord.id, editIn, editOut, editBreak);
      const calc = calculateWorkHours(editIn, editOut, editBreak);
      adjudicateAttendanceException(
        selectedRecord.id,
        "corrected",
        justificationNote || "출퇴근 누락 사후 보정",
        calc.netWorkHours ?? 8
      );
    } else {
      adjudicateAttendanceException(
        selectedRecord.id,
        resolutionType,
        justificationNote || "예외 심사 완료",
        selectedRecord.workedHours
      );
    }

    setAdjudicationModalOpen(false);
  };

  // 실시간 펀치 시뮬레이션
  const currentPunchCalc = calculateWorkHours(editIn, editOut, editBreak);

  const columns: Column<AttendanceRecord>[] = [
    {
      key: "employeeName",
      header: "성명",
      width: "100px",
      render: (row) => <span className="font-semibold">{row.employeeName}</span>,
    },
    {
      key: "site",
      header: "근무 현장",
      render: (row) => <span className="text-[var(--steel)]">{row.site}</span>,
    },
    {
      key: "planned",
      header: "계획 시각",
      width: "110px",
      render: (row) => (
        <span className="font-mono text-[11px] text-[var(--faint)]">
          {row.plannedIn} - {row.plannedOut}
        </span>
      ),
    },
    {
      key: "actual",
      header: "실적 펀치",
      width: "110px",
      render: (row) => (
        <span className="font-mono text-[11px] font-medium text-[var(--ink)]">
          {row.clockIn && row.clockOut ? `${row.clockIn} - ${row.clockOut}` : "미기록"}
        </span>
      ),
    },
    {
      key: "workedHours",
      header: "실근로 (휴게차감)",
      width: "110px",
      align: "right",
      render: (row) => (
        <span className="font-mono font-bold">
          {row.workedHours}h{" "}
          <span className="text-[10px] text-[var(--faint)] font-normal">
            (-{row.breakMinutes ?? 0}m)
          </span>
        </span>
      ),
    },
    {
      key: "weeklyHoursTotal",
      header: "주간 누적 / 주52h 규정",
      width: "180px",
      render: (row) => {
        const compliance = evaluate52HourCompliance(row.weeklyHoursTotal);
        return (
          <div className="space-y-1">
            <div className="flex items-center justify-between text-[11px]">
              <span className="font-mono font-bold">{row.weeklyHoursTotal}h / 52h</span>
              <StatusChip label={compliance.label} tone={compliance.tone} size="xs" />
            </div>
            <div className="w-full h-1.5 rounded-full bg-gray-200 overflow-hidden">
              <div
                className={`h-full ${
                  compliance.status === "VIOLATION"
                    ? "bg-red-600"
                    : compliance.status === "WARNING"
                    ? "bg-amber-500"
                    : "bg-emerald-500"
                }`}
                style={{ width: `${Math.min(100, (row.weeklyHoursTotal / 52) * 100)}%` }}
              />
            </div>
          </div>
        );
      },
    },
    {
      key: "exceptionCode",
      header: "예외 심사",
      width: "130px",
      render: (row) =>
        row.exceptionCode ? (
          <button
            onClick={() => handleOpenAdjudication(row)}
            className="cursor-pointer"
          >
            <ObjectLink code={row.exceptionCode} label="심사" />
          </button>
        ) : (
          <StatusChip label="통과" tone="ok" size="xs" />
        ),
    },
    {
      key: "action",
      header: "관리",
      width: "90px",
      render: (row) => (
        <Button size="xs" variant="secondary" onClick={() => handleOpenAdjudication(row)}>
          보정/심사
        </Button>
      ),
    },
  ];

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "당일 출근율", value: "87.5%", badge: "7/8명 출근", badgeTone: "ok" },
          { label: "주52h 초과 위험", value: "1명", badge: "위반 방지", badgeTone: "danger" },
          {
            label: "근태 마감 블로커",
            value: `${unresolvedCount}건`,
            badge: unresolvedCount > 0 ? "급여 락 발동중" : "게이트 통과",
            badgeTone: unresolvedCount > 0 ? "danger" : "ok",
          },
          { label: "휴게시간 자동적용", value: "근기법 §54", badge: "4h 30m / 8h 60m", badgeTone: "info" },
        ]}
      />

      {/* 2. 급여 마감 게이트 상태 안내 배너 */}
      <div
        className={`p-4 rounded-lg border flex flex-wrap items-center justify-between gap-3 ${
          unresolvedCount > 0
            ? "border-amber-300 bg-amber-50/80 text-amber-950"
            : "border-emerald-300 bg-emerald-50/80 text-emerald-950"
        }`}
      >
        <div className="flex items-center gap-3">
          {unresolvedCount > 0 ? (
            <AlertTriangle className="w-5 h-5 text-amber-600 shrink-0" />
          ) : (
            <CheckCircle2 className="w-5 h-5 text-emerald-600 shrink-0" />
          )}
          <div>
            <h4 className="text-xs font-bold">
              {unresolvedCount > 0
                ? `월 급여 마감 대기: 미해결 근태 예외 ${unresolvedCount}건 존재`
                : "근태 마감 게이트 통과 완료: 정기 급여 계산 및 마감 가능"}
            </h4>
            <p className="text-[11px] opacity-80">
              {unresolvedCount > 0
                ? "아래 테이블에서 예외(AT-*) 행의 '보정/심사' 버튼을 눌러 소명을 승인하면 급여 락이 자동으로 해제됩니다."
                : "모든 근로자의 근태가 합법적으로 소명·마감되었습니다. 지금 급여 계산을 확정할 수 있습니다."}
            </p>
          </div>
        </div>

        <Link href="/hr/payroll">
          <Button
            size="sm"
            variant={unresolvedCount === 0 ? "brand" : "secondary"}
            leftIcon={<ArrowRight className="w-3.5 h-3.5" />}
          >
            급여 계산실로 이동
          </Button>
        </Link>
      </div>

      {/* 2.5 모듈 레벨 관할 인가 범위 필터 */}
      <ModuleScopeFilter
        value={scopeFilter}
        onChange={setScopeFilter}
        filteredCount={filteredAttendance.length}
        totalCount={attendance.length}
        showDept={false}
      />

      {/* 3. 계획 vs 실적 2트랙 근태 매트릭스 테이블 */}
      <DataTable
        columns={columns}
        data={filteredAttendance}
        keyExtractor={(item) => item.id}
        onRowClick={(item) => handleOpenAdjudication(item)}
        searchPlaceholder="사원명, 현장, 예외코드(AT-) 검색..."
        searchFilter={(item, q) =>
          item.employeeName.toLowerCase().includes(q.toLowerCase()) ||
          item.site.toLowerCase().includes(q.toLowerCase()) ||
          Boolean(item.exceptionCode?.toLowerCase().includes(q.toLowerCase()))
        }
      />

      {/* 4. 근태 펀치 보정 & 52시간 예외 심사 모달 */}
      {adjudicationModalOpen && selectedRecord && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Clock className="w-5 h-5 text-orange-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">
                  근태 펀치 보정 및 예외 심사
                </h3>
              </div>
              <button
                onClick={() => setAdjudicationModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer"
              >
                ×
              </button>
            </div>

            <div className="space-y-3 text-xs">
              <div className="p-3 rounded border border-[var(--border)] bg-gray-50/50 space-y-1">
                <div className="flex justify-between font-semibold">
                  <span>{selectedRecord.employeeName} 사원</span>
                  <span className="font-mono text-[var(--steel)]">{selectedRecord.site}</span>
                </div>
                <div className="text-[11px] text-[var(--steel)]">
                  계획: {selectedRecord.plannedIn} ~ {selectedRecord.plannedOut}
                  {selectedRecord.exceptionCode && (
                    <span className="ml-2 font-mono text-red-600 font-bold">
                      [{selectedRecord.exceptionCode} 예외 발동]
                    </span>
                  )}
                </div>
              </div>

              {/* 실적 펀치 시간 입력 */}
              <div className="grid grid-cols-3 gap-2">
                <div>
                  <label className="font-semibold block mb-1">출근 시각</label>
                  <input
                    type="text"
                    value={editIn}
                    onChange={(e) => setEditIn(e.target.value)}
                    placeholder="09:00"
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono text-center"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">퇴근 시각</label>
                  <input
                    type="text"
                    value={editOut}
                    onChange={(e) => setEditOut(e.target.value)}
                    placeholder="18:00"
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono text-center"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">휴게(분)</label>
                  <input
                    type="number"
                    value={editBreak}
                    onChange={(e) => setEditBreak(Number(e.target.value))}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono text-center"
                  />
                </div>
              </div>

              {/* 근로기준법 제54조 실시간 휴게/실근로 계산 피드백 */}
              <div className="p-2.5 rounded border border-blue-200 bg-blue-50/60 text-blue-950 space-y-1">
                <div className="flex justify-between font-semibold text-[11px]">
                  <span>실시간 법정 산정 결과:</span>
                  <span className="font-mono">
                    실근로 {currentPunchCalc.netWorkHours ?? 0}시간 (휴게 -{currentPunchCalc.breakMinutes}분)
                  </span>
                </div>
                {currentPunchCalc.isMidnightShift && (
                  <div className="text-[10px] text-indigo-700 font-medium">
                    * 자정 경과 야간근로 감지 (심야 할증 대상)
                  </div>
                )}
              </div>

              {/* 심사 결정 방식 */}
              <div>
                <label className="font-semibold block mb-1">심사 판정 선택</label>
                <div className="grid grid-cols-3 gap-1.5">
                  <button
                    type="button"
                    onClick={() => setResolutionType("approved")}
                    className={`py-1.5 px-2 rounded border text-[11px] font-semibold cursor-pointer ${
                      resolutionType === "approved"
                        ? "border-emerald-500 bg-emerald-50 text-emerald-900"
                        : "border-[var(--border)]"
                    }`}
                  >
                    소명 승인 (인정)
                  </button>
                  <button
                    type="button"
                    onClick={() => setResolutionType("corrected")}
                    className={`py-1.5 px-2 rounded border text-[11px] font-semibold cursor-pointer ${
                      resolutionType === "corrected"
                        ? "border-blue-500 bg-blue-50 text-blue-900"
                        : "border-[var(--border)]"
                    }`}
                  >
                    펀치 시간 보정
                  </button>
                  <button
                    type="button"
                    onClick={() => setResolutionType("rejected")}
                    className={`py-1.5 px-2 rounded border text-[11px] font-semibold cursor-pointer ${
                      resolutionType === "rejected"
                        ? "border-red-500 bg-red-50 text-red-900"
                        : "border-[var(--border)]"
                    }`}
                  >
                    반려 (무단결근)
                  </button>
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">심사 사유 / 결재 링크</label>
                <input
                  type="text"
                  value={justificationNote}
                  onChange={(e) => setJustificationNote(e.target.value)}
                  placeholder="예: AP-3121 승인에 따른 선적 지연 인정"
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>
            </div>

            <div className="flex items-center justify-between pt-3 border-t border-[var(--border)]">
              <Button variant="ghost" size="sm" onClick={() => setAdjudicationModalOpen(false)}>
                취소
              </Button>
              <Button variant="brand" size="sm" onClick={executeAdjudication}>
                심사 확정 및 급여 게이트 해제
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
