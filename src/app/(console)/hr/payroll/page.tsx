"use client";

import React, { useState } from "react";
import Link from "next/link";
import { useAppStore } from "@/lib/store";
import { DataTable, Column } from "@/components/ui/DataTable";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Button } from "@/components/ui/Button";
import { StatBar } from "@/components/ui/StatBar";
import { Payslip, PayrollDiscrepancy, ConsequenceSummary } from "@/lib/types";
import { DiscrepancyDrawer } from "@/components/ui/DiscrepancyDrawer";
import { ConsequenceModal } from "@/components/ui/ConsequenceModal";
import {
  DollarSign,
  AlertTriangle,
  CheckCircle2,
  Lock,
  Printer,
  FileCheck,
  Building,
  CreditCard,
  Zap,
  Download,
  Copy,
  KeyRound,
  ShieldCheck,
  FileCode,
  Check,
  Send,
  Save,
  RotateCcw,
  ArrowRight,
  FileSpreadsheet,
} from "lucide-react";
import { ConnectedSheet } from "@/components/sheet/ConnectedSheet";
import { ProposedChange } from "@/lib/types";
import { ModuleScopeFilter, ModuleScopeState } from "@/components/gov/ModuleScopeFilter";

export default function PayrollPage() {
  const employees = useAppStore((s) => s.employees);
  const payrollRun = useAppStore((s) => s.payrollRun);
  const payslips = useAppStore((s) => s.payslips);
  const attendance = useAppStore((s) => s.attendance);
  const bankBatch = useAppStore((s) => s.bankTransferBatch);
  const discrepancies = useAppStore((s) => s.discrepancies);
  const payrollDraft = useAppStore((s) => s.payrollDraft);
  const calculateAndFreezePayroll = useAppStore((s) => s.calculateAndFreezePayroll);
  const generateFirmBankingBatch = useAppStore((s) => s.generateFirmBankingBatch);
  const sealAndDispatchBankBatch = useAppStore((s) => s.sealAndDispatchBankBatch);
  const repairDiscrepancy = useAppStore((s) => s.repairDiscrepancy);
  const savePayrollDraft = useAppStore((s) => s.savePayrollDraft);
  const resumePayrollDraft = useAppStore((s) => s.resumePayrollDraft);
  const updateEmployee = useAppStore((s) => s.updateEmployee);
  const viewAsRole = useAppStore((s) => s.viewAsRole);

  const [activeTab, setActiveTab] = useState<"payslip" | "sheet" | "banking">("payslip");
  const [bankingSubTab, setBankingSubTab] = useState<"kftc" | "openbanking">("kftc");
  const [selectedPayslip, setSelectedPayslip] = useState<Payslip | null>(payslips[0] || null);
  const [payslipModalOpen, setPayslipModalOpen] = useState(false);
  const [passkeyModalOpen, setPasskeyModalOpen] = useState(false);
  const [passkeyConsequenceOpen, setPasskeyConsequenceOpen] = useState(false);
  const [freezeError, setFreezeError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [passkeySigning, setPasskeySigning] = useState(false);

  // 모듈 관할 인가 범위 필터 상태 (법인/현장/부서)
  const [scopeFilter, setScopeFilter] = useState<ModuleScopeState>({
    entityId: "all",
    siteId: "all",
    deptId: "all",
  });

  // 사원 매핑 맵
  const empMap = React.useMemo(() => {
    return new Map(employees.map((e) => [e.id, e]));
  }, [employees]);

  // 관할 범위 필터링된 사원 목록
  const filteredEmployees = React.useMemo(() => {
    return employees.filter((e) => {
      if (scopeFilter.entityId !== "all" && e.entity !== scopeFilter.entityId) return false;
      if (scopeFilter.siteId !== "all" && e.site !== scopeFilter.siteId) return false;
      if (scopeFilter.deptId !== "all" && e.dept !== scopeFilter.deptId) return false;
      return true;
    });
  }, [employees, scopeFilter]);

  // 관할 범위 필터링된 급여명세서 대장
  const filteredPayslips = React.useMemo(() => {
    return payslips.filter((slip) => {
      const emp = empMap.get(slip.employeeId);
      if (!emp) return true;
      if (scopeFilter.entityId !== "all" && emp.entity !== scopeFilter.entityId) return false;
      if (scopeFilter.siteId !== "all" && emp.site !== scopeFilter.siteId) return false;
      if (scopeFilter.deptId !== "all" && emp.dept !== scopeFilter.deptId) return false;
      return true;
    });
  }, [payslips, empMap, scopeFilter]);

  // 관할 범위 필터링된 근태 기록
  const filteredAttendance = React.useMemo(() => {
    return attendance.filter((rec) => {
      const emp = empMap.get(rec.employeeId);
      if (!emp) return true;
      if (scopeFilter.entityId !== "all" && emp.entity !== scopeFilter.entityId) return false;
      if (scopeFilter.siteId !== "all" && emp.site !== scopeFilter.siteId) return false;
      if (scopeFilter.deptId !== "all" && emp.dept !== scopeFilter.deptId) return false;
      return true;
    });
  }, [attendance, empMap, scopeFilter]);

  // Discrepancy drawer state
  const [selectedDiscrepancy, setSelectedDiscrepancy] = useState<PayrollDiscrepancy | null>(null);
  const [discrepancyDrawerOpen, setDiscrepancyDrawerOpen] = useState(false);

  const openDiscrepancies = discrepancies.filter((d) => d.status === "unresolved");

  const handleRunCalculation = () => {
    setFreezeError(null);
    const res = calculateAndFreezePayroll();
    if (!res.success) {
      setFreezeError(res.error || "마감 게이트 검증 실패");
    } else {
      const latestSlips = useAppStore.getState().payslips;
      if (latestSlips.length > 0) setSelectedPayslip(latestSlips[0]);
    }
  };

  const handleOpenPayslip = (slip: Payslip) => {
    setSelectedPayslip(slip);
    setPayslipModalOpen(true);
  };

  const handleInspectDiscrepancy = (disc: PayrollDiscrepancy) => {
    setSelectedDiscrepancy(disc);
    setDiscrepancyDrawerOpen(true);
  };

  const handleRepairDiscrepancy = (id: string, note: string) => {
    repairDiscrepancy(id, note);
    // Reactively refresh payslip calculations if all blockers are cleared
    setTimeout(() => {
      const unres = useAppStore.getState().payrollRun.unresolvedExceptions;
      if (unres === 0) {
        calculateAndFreezePayroll();
      }
    }, 150);
  };

  const handleSaveDraft = () => {
    savePayrollDraft({
      activeTab,
      yearMonth: payrollRun.yearMonth || "2026-07",
    });
  };

  const handleResumeDraft = () => {
    const draft = resumePayrollDraft();
    if (draft) {
      setActiveTab(draft.activeTab as "payslip" | "sheet" | "banking");
    }
  };

  const handleCommitSheetChanges = (changes: ProposedChange[]) => {
    changes.forEach((change) => {
      if (change.columnId === "proposedAllowance") {
        updateEmployee(change.rowKey, { fixedAllow: Number(change.proposedValue) });
      }
    });
    setTimeout(() => {
      calculateAndFreezePayroll();
    }, 150);
  };

  const handleDownloadFlatFile = () => {
    if (!bankBatch?.kftcFlatFileContent) return;
    const blob = new Blob([bankBatch.kftcFlatFileContent], { type: "text/plain;charset=euc-kr" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `KFTC_CMS_PAYROLL_${bankBatch.yearMonth.replace("-", "")}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleCopyContent = (text: string) => {
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  // Consequence summary before passkey signing
  const passkeyConsequenceSummary: ConsequenceSummary = {
    title: "2026년 7월 정기 급여 펌뱅킹(CMS) 출금 승인 및 전자봉인",
    actionType: "대량급여지급",
    subject: `(주)오야티 코퍼레이션 대량 급여 이체 모계좌 (${bankBatch.masterBank} ${bankBatch.masterAccount})`,
    effectiveDate: "2026-07-28 즉시 (익영업일 09:00 타행공동망 정산 송금)",
    summaryLines: [
      `전체 임직원 ${bankBatch.totalHeadcount}명 대상 총 ${bankBatch.totalAmount.toLocaleString()}원 실지급액 확정 출금`,
      "금융결제원(KFTC) 기업 펌뱅킹 100-byte 표준 전문 및 전자서명 토큰 즉시 송신",
      "출금 승인 즉시 원장 상태가 '이체지시완료(FROZEN)'로 전환되며, 임의 변경이나 재산정이 영구 차단됨",
      "국고금관리법 제47조(10원 미만 절사) 및 4대보험 원천징수 예수금 법정 분리 회계 계정 전표 발행"
    ],
    historicalImpact: "본 이체 지시는 감사 해시체인(SHA-256)에 기록되며, FIDO2 하드웨어 토큰 서명값과 함께 영구 보존됩니다.",
    requiresApproval: true,
    approverRole: viewAsRole || "박지영 수석 (CFO 대행)",
    consequenceButtonLabel: "Passkey 생체 서명 및 최종 출금 승인",
  };

  const handleConfirmPasskeyConsequence = () => {
    setPasskeyConsequenceOpen(false);
    setPasskeyModalOpen(true);
  };

  const handleExecutePasskeySeal = () => {
    setPasskeySigning(true);
    setTimeout(() => {
      sealAndDispatchBankBatch(viewAsRole || "박지영 수석 (CFO 대행)");
      setPasskeySigning(false);
      setPasskeyModalOpen(false);
    }, 1200);
  };

  const columns: Column<Payslip>[] = [
    {
      key: "code",
      header: "명세 번호",
      width: "110px",
      render: (row) => <ObjectLink code={row.code} />,
    },
    {
      key: "employeeName",
      header: "성명",
      width: "130px",
      render: (row) => {
        const disc = discrepancies.find(
          (d) => d.employeeId === row.employeeId && d.status === "unresolved"
        );
        return (
          <div>
            <span className="font-semibold text-[var(--ink)]">{row.employeeName}</span>
            {disc && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  handleInspectDiscrepancy(disc);
                }}
                className="mt-1 flex items-center gap-1 text-[10px] text-amber-800 bg-amber-100 hover:bg-amber-200 px-1.5 py-0.5 rounded border border-amber-300 font-medium cursor-pointer transition-colors"
                title="근태 예외 심사 드로어 열기"
              >
                <AlertTriangle className="w-2.5 h-2.5 text-amber-600 shrink-0" />
                <span>{disc.sourceCode} 예외 심사</span>
              </button>
            )}
          </div>
        );
      },
    },
    {
      key: "empType",
      header: "고용 형태",
      width: "80px",
      render: (row) => <StatusChip label={row.empType} size="xs" />,
    },
    {
      key: "basePay",
      header: "기본급",
      align: "right",
      render: (row) => <span className="font-mono">{row.basePay.toLocaleString()}원</span>,
    },
    {
      key: "overtimePay",
      header: "연장/야간 수당",
      align: "right",
      render: (row) => (
        <span className="font-mono text-blue-700">
          {(row.overtimePay + row.nightPay).toLocaleString()}원
        </span>
      ),
    },
    {
      key: "grossPay",
      header: "지급 총액 (과세)",
      align: "right",
      render: (row) => (
        <span className="font-mono font-bold text-[var(--ink)]">
          {row.grossPay.toLocaleString()}원
        </span>
      ),
    },
    {
      key: "totalDeductions",
      header: "법정 공제액 (4대보험/세금)",
      align: "right",
      render: (row) => (
        <span className="font-mono text-red-600">
          -{row.totalDeductions.toLocaleString()}원
        </span>
      ),
    },
    {
      key: "netPay",
      header: "실지급액 (Net)",
      align: "right",
      render: (row) => (
        <span className="font-mono font-bold text-emerald-700 text-sm">
          {row.netPay.toLocaleString()}원
        </span>
      ),
    },
    {
      key: "action",
      header: "명세서",
      width: "80px",
      render: (row) => (
        <Button size="xs" variant="secondary" onClick={() => handleOpenPayslip(row)}>
          열람
        </Button>
      ),
    },
  ];

  return (
    <div className="space-y-5">
      {/* 탭 네비게이션 */}
      <div className="flex items-center justify-between border-b border-[var(--border)] pb-2">
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setActiveTab("payslip")}
            className={`flex items-center gap-1.5 px-3 py-1.5 text-xs font-semibold rounded-md transition-colors cursor-pointer ${
              activeTab === "payslip"
                ? "bg-[var(--signal-soft)] text-[var(--signal-deep)] border border-amber-300"
                : "text-[var(--steel)] hover:bg-[var(--muted)]"
            }`}
          >
            <DollarSign className="w-3.5 h-3.5" />
            <span>임금명세서 대장 (List)</span>
            <span className="px-1.5 py-0.2 rounded-full text-[10px] font-mono bg-white/70">
              {payslips.length}
            </span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("sheet")}
            className={`flex items-center gap-1.5 px-3 py-1.5 text-xs font-semibold rounded-md transition-colors cursor-pointer ${
              activeTab === "sheet"
                ? "bg-emerald-50 text-emerald-900 border border-emerald-300 font-bold"
                : "text-[var(--steel)] hover:bg-[var(--muted)]"
            }`}
          >
            <FileSpreadsheet className="w-3.5 h-3.5 text-emerald-700" />
            <span>급여 기안 시트 (Connected Sheet)</span>
            <span className="px-1.5 py-0.2 rounded-full text-[10px] font-mono bg-emerald-100 text-emerald-800 font-bold">
              Grid
            </span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("banking")}
            className={`flex items-center gap-1.5 px-3 py-1.5 text-xs font-semibold rounded-md transition-colors cursor-pointer ${
              activeTab === "banking"
                ? "bg-[var(--signal-soft)] text-[var(--signal-deep)] border border-amber-300"
                : "text-[var(--steel)] hover:bg-[var(--muted)]"
            }`}
          >
            <CreditCard className="w-3.5 h-3.5" />
            <span>은행 이체 및 펌뱅킹 (CMS)</span>
            <span className="px-1.5 py-0.2 rounded-full text-[10px] font-mono bg-emerald-100 text-emerald-800">
              {bankBatch.status}
            </span>
          </button>
        </div>

        {(activeTab === "payslip" || activeTab === "sheet") && (
          <div className="flex items-center gap-2">
            {payrollDraft && (
              <Button
                size="xs"
                variant="secondary"
                leftIcon={<RotateCcw className="w-3 h-3 text-blue-600" />}
                onClick={handleResumeDraft}
                title={`저장일시: ${payrollDraft.savedAt}`}
              >
                현재 탭 임시 보관 ({payrollDraft.savedAt.slice(11)})
              </Button>
            )}
            <Button
              size="xs"
              variant="secondary"
              leftIcon={<Save className="w-3 h-3" />}
              onClick={handleSaveDraft}
            >
              이 탭에 임시 보관
            </Button>
          </div>
        )}

        {activeTab === "banking" && (
          <div className="flex items-center gap-2">
            <Button
              size="xs"
              variant="secondary"
              leftIcon={<Zap className="w-3 h-3" />}
              onClick={() => generateFirmBankingBatch()}
            >
              전문 재생성
            </Button>
            <Button
              size="xs"
              variant="secondary"
              leftIcon={<Download className="w-3 h-3" />}
              onClick={handleDownloadFlatFile}
            >
              KFTC 전문(.txt) 다운로드
            </Button>
            {bankBatch.status !== "출금승인대기" && bankBatch.status !== "이체지시완료" ? (
              <Button
                size="xs"
                variant="brand"
                leftIcon={<KeyRound className="w-3 h-3" />}
                onClick={() => setPasskeyConsequenceOpen(true)}
              >
                Passkey 출금 승인 및 봉인
              </Button>
            ) : (
              <span className="inline-flex items-center gap-1 text-[11px] font-mono font-bold text-emerald-800 bg-emerald-50 px-2 py-1 rounded border border-emerald-200">
                <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
                <span>Passkey 봉인 완료</span>
              </span>
            )}
          </div>
        )}
      </div>

      {activeTab === "payslip" ? (
        <>
          {/* 1. 상단 스탯 바 */}
          <StatBar
            items={[
              { label: "급여 회차", value: payrollRun.code, subValue: "2026-07 월급여" },
              { label: "대상 인원", value: `${employees.length}명` },
              { label: "지급 총액 (Gross)", value: `${payrollRun.totalGross.toLocaleString()}원` },
              { label: "법정 공제 합계", value: `-${payrollRun.totalDeductions.toLocaleString()}원`, badgeTone: "danger" },
              { label: "실지급 총액 (Net)", value: `${payrollRun.totalNet.toLocaleString()}원`, badgeTone: "ok" },
            ]}
          />

          {/* 급여 마감 블로커 및 근태 예외 콕핏 배너 */}
          {openDiscrepancies.length > 0 && (
            <div className="p-3.5 rounded-lg border border-amber-300 bg-amber-50/80 shadow-2xs space-y-2.5">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex items-center gap-2 text-xs font-bold text-amber-900">
                  <AlertTriangle className="w-4 h-4 text-amber-600 shrink-0" />
                  <span>급여 마감 차단 근태 및 법정 예외 ({openDiscrepancies.length}건 미해결)</span>
                </div>
                <span className="text-[11px] text-amber-800">
                  원클릭 소명 심사 및 인가 시, 급여 원장에 실시간 반영되어 즉시 마감 가능합니다.
                </span>
              </div>
              <div className="flex flex-wrap gap-2">
                {openDiscrepancies.map((disc) => (
                  <button
                    key={disc.id}
                    type="button"
                    onClick={() => handleInspectDiscrepancy(disc)}
                    className="flex items-center gap-2 px-3 py-1.5 rounded-md bg-white border border-amber-300 text-xs text-amber-950 font-medium hover:bg-amber-100/70 hover:border-amber-400 cursor-pointer shadow-2xs transition-all text-left group"
                  >
                    <span className="font-mono font-bold text-[10px] bg-amber-200 text-amber-900 px-1.5 py-0.5 rounded">
                      {disc.sourceCode}
                    </span>
                    <span className="font-bold">{disc.employeeName}</span>
                    <span className="text-[11px] text-amber-800">[{disc.statutoryRule}]</span>
                    <ArrowRight className="w-3.5 h-3.5 text-amber-600 ml-1 shrink-0 group-hover:translate-x-0.5 transition-transform" />
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* 2. 게이트 판정 및 1-클릭 급여 계산 컨트롤 바 */}
          <div className="p-4 rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs flex flex-wrap items-center justify-between gap-4">
            <div className="space-y-1">
              <div className="flex items-center gap-2">
                <span className="font-mono text-xs font-bold text-[var(--signal-deep)]">{payrollRun.code}</span>
                <StatusChip label={payrollRun.status} size="xs" dot />
                {payrollRun.isLocked && (
                  <span className="inline-flex items-center gap-1 text-[11px] font-mono text-emerald-700 bg-emerald-50 px-2 py-0.5 rounded border border-emerald-200">
                    <Lock className="w-3 h-3" />
                    <span>원장 동결 (Locked)</span>
                  </span>
                )}
              </div>
              <p className="text-xs text-[var(--steel)]">
                국고금관리법 제47조(10원 단위 절사) · 2026년도 국민연금(4.75%) · 건강보험(7.19%) · 장기요양(9448/71900) 법정 커널 가동
              </p>
            </div>

            {/* 1-Click Game-like Execution Action */}
            <div className="flex items-center gap-2">
              {payrollRun.unresolvedExceptions > 0 ? (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-amber-700 font-medium flex items-center gap-1">
                    <AlertTriangle className="w-4 h-4" />
                    근태 예외 {payrollRun.unresolvedExceptions}건 미해결
                  </span>
                  {openDiscrepancies.length > 0 && (
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => handleInspectDiscrepancy(openDiscrepancies[0])}
                    >
                      즉시 소명 심사 ({openDiscrepancies[0].sourceCode})
                    </Button>
                  )}
                  <Link href="/hr/attendance">
                    <Button size="sm" variant="brand">
                      근태 예외 심사하러 가기 →
                    </Button>
                  </Link>
                </div>
              ) : (
                <Button
                  size="sm"
                  variant="brand"
                  onClick={handleRunCalculation}
                  leftIcon={<Zap className="w-3.5 h-3.5" />}
                >
                  {payrollRun.isLocked ? "급여 재계산 및 갱신" : "7월 급여 실시간 계산 및 마감 실행"}
                </Button>
              )}
            </div>
          </div>

          {/* 에러 피드백 배너 */}
          {freezeError && (
            <div className="p-3 rounded-md border border-red-300 bg-red-50 text-red-900 text-xs flex items-center gap-2 animate-in fade-in duration-150">
              <AlertTriangle className="w-4 h-4 text-red-600 shrink-0" />
              <span>{freezeError}</span>
            </div>
          )}

          {/* 모듈 레벨 관할 인가 범위 필터 */}
          <ModuleScopeFilter
            value={scopeFilter}
            onChange={setScopeFilter}
            filteredCount={filteredPayslips.length}
            totalCount={payslips.length}
          />

          {/* 3. 명세서 대장 (DataTable) */}
          <DataTable
            columns={columns}
            data={filteredPayslips}
            keyExtractor={(item) => item.id}
            onRowClick={(item) => handleOpenPayslip(item)}
            searchPlaceholder="사원명, 명세서코드(PS-) 검색..."
            searchFilter={(item, q) =>
              item.employeeName.toLowerCase().includes(q.toLowerCase()) ||
              item.code.toLowerCase().includes(q.toLowerCase())
            }
          />
        </>
      ) : activeTab === "sheet" ? (
        <div className="space-y-4">
          {/* 모듈 레벨 관할 인가 범위 필터 (급여 기안 시트 연동) */}
          <ModuleScopeFilter
            value={scopeFilter}
            onChange={setScopeFilter}
            filteredCount={filteredEmployees.length}
            totalCount={employees.length}
          />
          <ConnectedSheet
            employees={filteredEmployees}
            attendance={filteredAttendance}
            payslips={filteredPayslips}
            onCommitChanges={handleCommitSheetChanges}
            onSaveDraft={() => {
              savePayrollDraft({
                activeTab: "sheet",
                yearMonth: payrollRun.yearMonth,
              });
            }}
            onSelectEmployee={(emp) => {
              const slip = payslips.find((p) => p.employeeId === emp.id);
              if (slip) setSelectedPayslip(slip);
            }}
          />
        </div>
      ) : (
        /* 은행 이체 & 펌뱅킹 (Firm Banking / CMS) 덱 */
        <div className="space-y-4">
          {/* 법인 출금 모계좌 및 종합 상태 배너 */}
          <div className="p-4 rounded-lg border border-[var(--border)] bg-gradient-to-r from-blue-50/40 via-surface to-amber-50/20 shadow-2xs space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-3">
                <div className="w-10 h-10 rounded-lg bg-blue-600 text-white flex items-center justify-center font-black shadow-xs">
                  <Building className="w-5 h-5" />
                </div>
                <div>
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-bold text-[var(--ink)]">
                      (주)오야티 코퍼레이션 대량 급여 이체 모계좌
                    </span>
                    <span className="font-mono text-xs text-blue-700 bg-blue-100/70 px-2 py-0.5 rounded font-semibold">
                      {bankBatch.masterBank} {bankBatch.masterAccount}
                    </span>
                    <StatusChip label={bankBatch.status} size="xs" dot />
                  </div>
                  <div className="text-xs text-[var(--steel)] mt-0.5">
                    금융결제원(KFTC) 기업 펌뱅킹 CMS 대량 이체 프로토콜 v2.6 · 타행공동망 즉시 정산 연동
                  </div>
                </div>
              </div>

              <div className="flex items-center gap-4 text-right">
                <div>
                  <div className="text-[10px] text-[var(--faint)]">총 실지급 이체액</div>
                  <div className="text-lg font-black font-mono text-emerald-800">
                    {bankBatch.totalAmount.toLocaleString()}원
                  </div>
                </div>
                <div className="border-l border-[var(--border)] pl-4">
                  <div className="text-[10px] text-[var(--faint)]">이체 대상 인원</div>
                  <div className="text-lg font-black font-mono text-[var(--ink)]">
                    {bankBatch.totalHeadcount}명
                  </div>
                </div>
              </div>
            </div>

            {/* 사전 검증 및 해시 체인 상태 */}
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
              <div className="p-2.5 rounded border border-emerald-200 bg-emerald-50/60 flex items-center gap-2">
                <CheckCircle2 className="w-4 h-4 text-emerald-700 shrink-0" />
                <div>
                  <span className="font-bold text-emerald-900 block">수취계좌 사전 실명검증</span>
                  <span className="text-[11px] text-emerald-800">
                    전산 조회 결과 100% 정상 (예금주명 일치 확인 완료)
                  </span>
                </div>
              </div>
              <div className="p-2.5 rounded border border-blue-200 bg-blue-50/60 flex items-center gap-2">
                <FileCheck className="w-4 h-4 text-blue-700 shrink-0" />
                <div>
                  <span className="font-bold text-blue-900 block">이체 기준일 & 전문 코드</span>
                  <span className="text-[11px] text-blue-800 font-mono">
                    {bankBatch.paymentDate} · {bankBatch.code}
                  </span>
                </div>
              </div>
              <div className="p-2.5 rounded border border-[var(--border)] bg-gray-50 flex items-center gap-2">
                <Lock className="w-4 h-4 text-[var(--steel)] shrink-0" />
                <div className="truncate">
                  <span className="font-bold text-[var(--ink)] block">출금 전자서명 상태</span>
                  <span className="text-[11px] text-[var(--steel)] truncate font-mono">
                    {bankBatch.sealedBy || "미서명 (Passkey 승인 대기)"}
                  </span>
                </div>
              </div>
            </div>
          </div>

          {/* 은행별 집계 카드 그리드 */}
          <div className="space-y-1.5">
            <h3 className="text-xs font-bold text-[var(--steel)] uppercase tracking-wider">
              은행 금융기관별 이체 집계표 (Clearing Aggregation)
            </h3>
            <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-2.5">
              {bankBatch.bankSummaries.map((b) => (
                <div
                  key={b.bankCode}
                  className="p-3 rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs hover:border-blue-400 transition-colors"
                >
                  <div className="flex items-center justify-between text-xs mb-1">
                    <span className="font-bold text-[var(--ink)]">{b.bankName}</span>
                    <span className="font-mono text-[10px] text-[var(--faint)] bg-gray-100 px-1 py-0.2 rounded">
                      {b.bankCode}
                    </span>
                  </div>
                  <div className="text-sm font-black font-mono text-[var(--ink)]">
                    {b.amount.toLocaleString()}원
                  </div>
                  <div className="flex items-center justify-between text-[10px] text-[var(--steel)] mt-1.5 pt-1.5 border-t border-[var(--border-soft)]">
                    <span>{b.count}건 지급</span>
                    <span className="text-emerald-700 font-medium">수수료 0원</span>
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* 전문 및 페이로드 인스펙터 */}
          <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs overflow-hidden">
            <div className="flex items-center justify-between px-4 py-2.5 bg-gray-50/80 border-b border-[var(--border)]">
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => setBankingSubTab("kftc")}
                  className={`px-2.5 py-1 rounded text-xs font-bold transition-colors cursor-pointer ${
                    bankingSubTab === "kftc"
                      ? "bg-white text-[var(--ink)] border border-[var(--border)] shadow-2xs"
                      : "text-[var(--steel)] hover:text-[var(--ink)]"
                  }`}
                >
                  금융결제원(KFTC) 100-byte 표준 전문 파일
                </button>
                <button
                  type="button"
                  onClick={() => setBankingSubTab("openbanking")}
                  className={`px-2.5 py-1 rounded text-xs font-bold transition-colors cursor-pointer ${
                    bankingSubTab === "openbanking"
                      ? "bg-white text-[var(--ink)] border border-[var(--border)] shadow-2xs"
                      : "text-[var(--steel)] hover:text-[var(--ink)]"
                  }`}
                >
                  오픈뱅킹 REST API 전송 페이로드 (JSON)
                </button>
              </div>

              <div className="flex items-center gap-2">
                <Button
                  size="xs"
                  variant="secondary"
                  leftIcon={copied ? <Check className="w-3 h-3 text-emerald-600" /> : <Copy className="w-3 h-3" />}
                  onClick={() =>
                    handleCopyContent(
                      bankingSubTab === "kftc"
                        ? bankBatch.kftcFlatFileContent
                        : JSON.stringify(bankBatch.openApiPayload, null, 2)
                    )
                  }
                >
                  {copied ? "복사완료!" : "전문 복사"}
                </Button>
              </div>
            </div>

            {bankingSubTab === "kftc" ? (
              <div className="p-4 space-y-3">
                {/* 바이트 규격 범례 설명 */}
                <div className="p-2.5 rounded bg-amber-50/70 border border-amber-200 text-[11px] text-amber-900 leading-relaxed">
                  <span className="font-bold block mb-0.5">
                    ※ KFTC 고정길이 100-byte 레코드 레이아웃 규격 명세:
                  </span>
                  <div className="grid grid-cols-1 md:grid-cols-3 gap-2 font-mono text-[10px]">
                    <div>
                      <span className="font-bold text-blue-800">[H] 헤더 레코드 (100B)</span>: 구분(1) + 기관코드(10) + 파일일자(8) + 출금일자(8) + 모계좌(20) + 총건수(6) + 총금액(13)
                    </div>
                    <div>
                      <span className="font-bold text-emerald-800">[D] 데이터 레코드 (100B)</span>: 구분(1) + 순번(6) + 은행코드(3) + 입금계좌(20) + 이체금액(10) + 수취인(10) + 적요(10)
                    </div>
                    <div>
                      <span className="font-bold text-purple-800">[T] 트레일러 레코드 (100B)</span>: 구분(1) + 총이체건수(6) + 총이체금액(13) + 공백패딩(74)
                    </div>
                  </div>
                </div>

                {/* Flat File Content View */}
                <div className="rounded border border-gray-700 bg-slate-950 p-3 font-mono text-xs text-emerald-400 overflow-x-auto select-all leading-loose">
                  {bankBatch.kftcFlatFileContent.split("\n").map((line, idx) => (
                    <div key={idx} className="flex gap-4 hover:bg-slate-900 px-1 rounded">
                      <span className="text-gray-500 select-none w-6 text-right shrink-0">{idx + 1}</span>
                      <span className="shrink-0">{line}</span>
                      <span className="text-gray-600 select-none text-[10px] pl-2">({line.length} bytes)</span>
                    </div>
                  ))}
                </div>
              </div>
            ) : (
              <div className="p-4 space-y-3">
                <div className="flex items-center justify-between text-xs text-[var(--steel)]">
                  <div className="flex items-center gap-2">
                    <span className="font-bold font-mono px-1.5 py-0.5 rounded bg-emerald-100 text-emerald-800">
                      POST
                    </span>
                    <span className="font-mono text-xs font-semibold text-[var(--ink)]">
                      https://openapi.kftc.or.kr/v2.0/transfer/withdraw
                    </span>
                  </div>
                  <span className="font-mono text-[11px] text-[var(--faint)]">
                    Content-Type: application/json; charset=UTF-8
                  </span>
                </div>

                <pre className="rounded border border-gray-700 bg-slate-950 p-3 font-mono text-xs text-blue-300 overflow-x-auto select-all max-h-96">
                  {JSON.stringify(bankBatch.openApiPayload, null, 2)}
                </pre>
              </div>
            )}
          </div>
        </div>
      )}

      {/* 4. 법정 근로기준법 급여명세서 상세 모달 */}
      {payslipModalOpen && selectedPayslip && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-xl rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 space-y-4 animate-in fade-in zoom-in-95 duration-150 max-h-[90vh] overflow-y-auto">
            {/* 상단 헤더 */}
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <div className="w-7 h-7 rounded-md bg-emerald-500/15 text-emerald-700 flex items-center justify-center font-bold">
                  ₩
                </div>
                <div>
                  <h3 className="text-sm font-bold text-[var(--ink)]">
                    근로기준법 제48조 표준 임금명세서
                  </h3>
                  <div className="text-[11px] text-[var(--steel)]">
                    {selectedPayslip.yearMonth}분기 귀속 정기 급여
                  </div>
                </div>
              </div>
              <button
                type="button"
                onClick={() => setPayslipModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer text-base leading-none"
              >
                ×
              </button>
            </div>

            {/* 인적사항 헤더 표 */}
            <div className="grid grid-cols-3 gap-2 p-2.5 rounded border border-[var(--border)] bg-gray-50/50 text-xs">
              <div>
                <span className="text-[var(--steel)] block text-[10px]">성명</span>
                <span className="font-bold">{selectedPayslip.employeeName}</span>
              </div>
              <div>
                <span className="text-[var(--steel)] block text-[10px]">명세서 고유번호</span>
                <span className="font-mono">{selectedPayslip.code}</span>
              </div>
              <div>
                <span className="text-[var(--steel)] block text-[10px]">고용 구분</span>
                <span>{selectedPayslip.empType}직</span>
              </div>
            </div>

            {/* 지급 및 공제 내역 2열 그리드 */}
            <div className="grid grid-cols-2 gap-4 text-xs">
              {/* 지급 항목 */}
              <div className="space-y-2 border border-[var(--border)] rounded p-3 bg-[var(--surface)]">
                <div className="font-bold text-emerald-800 border-b pb-1">지급 항목 (Earnings)</div>
                <div className="space-y-1.5 divide-y divide-[var(--border-soft)]">
                  <div className="flex justify-between pt-1">
                    <span className="text-[var(--steel)]">기본급</span>
                    <span className="font-mono">{selectedPayslip.basePay.toLocaleString()}원</span>
                  </div>
                  <div className="flex justify-between pt-1">
                    <span className="text-[var(--steel)]">직책·정액수당</span>
                    <span className="font-mono">{selectedPayslip.fixedAllowance.toLocaleString()}원</span>
                  </div>
                  {selectedPayslip.overtimePay > 0 && (
                    <div className="flex justify-between pt-1">
                      <span className="text-blue-700">연장근로수당 (1.5x)</span>
                      <span className="font-mono text-blue-800 font-semibold">
                        {selectedPayslip.overtimePay.toLocaleString()}원
                      </span>
                    </div>
                  )}
                  {selectedPayslip.nightPay > 0 && (
                    <div className="flex justify-between pt-1">
                      <span className="text-blue-700">야간근로수당 (0.5x)</span>
                      <span className="font-mono text-blue-800 font-semibold">
                        {selectedPayslip.nightPay.toLocaleString()}원
                      </span>
                    </div>
                  )}
                  {selectedPayslip.holidayPay > 0 && (
                    <div className="flex justify-between pt-1">
                      <span className="text-blue-700">휴일근로수당 (1.5x)</span>
                      <span className="font-mono text-blue-800 font-semibold">
                        {selectedPayslip.holidayPay.toLocaleString()}원
                      </span>
                    </div>
                  )}
                  <div className="flex justify-between pt-2 border-t border-[var(--border)] font-bold text-sm">
                    <span>지급액 합계</span>
                    <span className="font-mono">{selectedPayslip.grossPay.toLocaleString()}원</span>
                  </div>
                </div>
              </div>

              {/* 공제 항목 */}
              <div className="space-y-2 border border-[var(--border)] rounded p-3 bg-[var(--surface)]">
                <div className="font-bold text-red-800 border-b pb-1">공제 항목 (Deductions)</div>
                <div className="space-y-1.5 divide-y divide-[var(--border-soft)]">
                  {selectedPayslip.deductions.map((ded) => (
                    <div key={ded.code} className="flex justify-between pt-1">
                      <div>
                        <span className="text-[var(--steel)] block">{ded.label}</span>
                        <span className="text-[9px] text-[var(--faint)] block leading-tight">{ded.basis}</span>
                      </div>
                      <span className="font-mono text-red-700">-{ded.amt.toLocaleString()}원</span>
                    </div>
                  ))}
                  <div className="flex justify-between pt-2 border-t border-[var(--border)] font-bold text-sm text-red-700">
                    <span>공제액 합계</span>
                    <span className="font-mono">-{selectedPayslip.totalDeductions.toLocaleString()}원</span>
                  </div>
                </div>
              </div>
            </div>

            {/* 최종 차인지급액 (Net Pay) 배너 */}
            <div className="p-3 rounded-lg border border-emerald-300 bg-emerald-50 flex items-center justify-between">
              <div>
                <span className="text-xs font-bold text-emerald-900 block">실제 입금 예정액 (차인지급액)</span>
                <span className="text-[10px] text-emerald-700 font-mono">10원 미만 국고금관리법 제47조 적용 절사</span>
              </div>
              <div className="text-xl font-black font-mono text-emerald-950">
                {selectedPayslip.netPay.toLocaleString()}원
              </div>
            </div>

            <div className="flex items-center justify-between pt-2 border-t border-[var(--border)]">
              <Button
                variant="secondary"
                size="sm"
                leftIcon={<Printer className="w-3.5 h-3.5" />}
                onClick={() => window.print()}
              >
                명세서 인쇄 (A4 서식)
              </Button>
              <Button variant="brand" size="sm" onClick={() => setPayslipModalOpen(false)}>
                확인 닫기
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 5. FIDO2 Passkey 전자서명 출금 지시 승인 모달 */}
      {passkeyModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center gap-3 border-b border-[var(--border)] pb-3">
              <div className="w-10 h-10 rounded-full bg-blue-100 text-blue-700 flex items-center justify-center font-bold">
                <KeyRound className="w-5 h-5" />
              </div>
              <div>
                <h3 className="text-sm font-bold text-[var(--ink)]">
                  FIDO2 Passkey 출금 지시 승인 및 전자봉인
                </h3>
                <p className="text-xs text-[var(--steel)]">
                  금융결제원 CMS 대량 이체 전송 전 최고재무책임자(CFO) 결재
                </p>
              </div>
            </div>

            <div className="p-3 rounded-lg bg-gray-50 border border-[var(--border)] space-y-2 text-xs">
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">출금 모계좌:</span>
                <span className="font-mono font-bold">
                  {bankBatch.masterBank} {bankBatch.masterAccount}
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">총 이체 인원:</span>
                <span className="font-mono font-bold">{bankBatch.totalHeadcount}명</span>
              </div>
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">총 출금 집행액:</span>
                <span className="font-mono font-bold text-emerald-800 text-sm">
                  {bankBatch.totalAmount.toLocaleString()}원
                </span>
              </div>
              <div className="flex justify-between border-t border-[var(--border-soft)] pt-2">
                <span className="text-[var(--steel)]">승인 결재자:</span>
                <span className="font-semibold text-blue-800">{viewAsRole}</span>
              </div>
            </div>

            <div className="p-2.5 rounded bg-blue-50/80 border border-blue-200 text-[11px] text-blue-900 flex items-center gap-2">
              <ShieldCheck className="w-4 h-4 text-blue-600 shrink-0" />
              <span>
                본 승인은 금융회사 보안 규정에 따라 FIDO2 Passkey 하드웨어 보안 키로 서명되며, 변경 불가능한 감사 해시체인에 영구 기록됩니다.
              </span>
            </div>

            <div className="flex items-center justify-end gap-2 pt-2 border-t border-[var(--border)]">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setPasskeyModalOpen(false)}
                disabled={passkeySigning}
              >
                취소
              </Button>
              <Button
                variant="brand"
                size="sm"
                leftIcon={passkeySigning ? <Zap className="w-3.5 h-3.5 animate-spin" /> : <Send className="w-3.5 h-3.5" />}
                onClick={handleExecutePasskeySeal}
                disabled={passkeySigning}
              >
                {passkeySigning ? "생체 인증 서명 중..." : "Passkey 생체 서명 및 이체 승인"}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 6. 블로커 근태 예외 심사 드로어 */}
      <DiscrepancyDrawer
        isOpen={discrepancyDrawerOpen}
        onClose={() => setDiscrepancyDrawerOpen(false)}
        discrepancy={selectedDiscrepancy}
        onRepair={handleRepairDiscrepancy}
      />

      {/* 7. Passkey 이체 승인 전 비즈니스 영향도 (Consequences) 사전 검토 모달 */}
      <ConsequenceModal
        isOpen={passkeyConsequenceOpen}
        onClose={() => setPasskeyConsequenceOpen(false)}
        onConfirm={handleConfirmPasskeyConsequence}
        summary={passkeyConsequenceSummary}
      />
    </div>
  );
}
