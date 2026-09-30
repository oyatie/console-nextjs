"use client";

import React, { useState, useEffect, useRef, useMemo } from "react";
import {
  CellRole,
  SheetColumn,
  ProposedChange,
  SheetCellCoordinate,
  ConsequenceSummary,
  Employee,
  AttendanceRecord,
  Payslip,
} from "@/lib/types";
import {
  parseMoneyInput,
  parseDateInput,
  parseTsvClipboard,
  evaluateSheetFormula,
  matchObjectReference,
  summarizeProposedAllowanceChanges,
  DisambiguationCandidate,
} from "@/lib/sheet-engine";
import { Button } from "@/components/ui/Button";
import { ConsequenceModal } from "@/components/ui/ConsequenceModal";
import {
  Table,
  Calculator,
  Lock,
  Edit3,
  Sparkles,
  Link as LinkIcon,
  HelpCircle,
  Save,
  CheckCircle2,
  AlertTriangle,
  RotateCcw,
  ArrowRight,
  Plus,
  Layers,
  FileSpreadsheet,
  Info,
  Check,
  Building2,
  Calendar,
} from "lucide-react";

interface ConnectedSheetProps {
  employees: Employee[];
  attendance: AttendanceRecord[];
  payslips: Payslip[];
  onCommitChanges: (changes: ProposedChange[]) => void;
  onSaveDraft?: (changes: ProposedChange[]) => void;
  onSelectEmployee?: (employee: Employee) => void;
}

export function ConnectedSheet({
  employees,
  attendance,
  payslips,
  onCommitChanges,
  onSaveDraft,
  onSelectEmployee,
}: ConnectedSheetProps) {
  // 1. Grid Active Cell & Selection State
  const [activeCell, setActiveCell] = useState<SheetCellCoordinate>({
    rowKey: employees[0]?.id || "",
    colIndex: 4, // Default to proposedAllowance
    columnId: "proposedAllowance",
  });
  const [isEditing, setIsEditing] = useState(false);
  const [editValue, setEditValue] = useState("");
  const [formulaBarValue, setFormulaBarValue] = useState("");

  // 2. Proposed Changes Store (Keyed by `${rowKey}:${columnId}`)
  const [proposedChanges, setProposedChanges] = useState<Map<string, ProposedChange>>(new Map());

  // 3. User Added Analytical Columns
  const [analyticalColumns, setAnalyticalColumns] = useState<SheetColumn[]>([
    {
      id: "scenarioDelta",
      header: "시나리오 변동액",
      role: "sheet-formula",
      width: 140,
      type: "formula",
      formula: "=[@proposedAllowance] - [@fixedAllow]",
      description: "기존 고정 수당 대비 제안 수당 차액 계산",
      provenance: "분석용 인라인 수식 ([@proposedAllowance] - [@fixedAllow])",
      getValue: (r) => {
        const prop = proposedChanges.get(`${r.id}:proposedAllowance`);
        const pVal = prop ? prop.proposedValue : r.fixedAllow || 0;
        return (pVal || 0) - (r.fixedAllow || 0);
      },
      format: (val) => {
        const num = Number(val) || 0;
        return num > 0 ? `+${num.toLocaleString()}원` : num < 0 ? `-${Math.abs(num).toLocaleString()}원` : "0원";
      },
      isEditable: false,
    },
    {
      id: "plannerNote",
      header: "검토 의견 (Planner Note)",
      role: "analytical-input",
      width: 180,
      type: "text",
      description: "고용 원장 스키마 변경 없는 현장 플래너 로컬 검토 메모",
      provenance: "시트 소유 분석 필드 (Durable Local State)",
      getValue: (r) => {
        const note = proposedChanges.get(`${r.id}:plannerNote`);
        return note ? note.proposedValue : "";
      },
      format: (val) => val || "—",
      isEditable: true,
    },
  ]);

  // 4. Modal & Toast States
  const [consequenceModalOpen, setConsequenceModalOpen] = useState(false);
  const [cellInspectorOpen, setCellInspectorOpen] = useState(false);
  const [pasteToast, setPasteToast] = useState<string | null>(null);
  const [snapshotMode, setSnapshotMode] = useState<"live" | "snapshot">("live");

  const gridContainerRef = useRef<HTMLDivElement>(null);
  const cellInputRef = useRef<HTMLInputElement>(null);

  // 5. Build Authoritative Sheet Columns
  const baseColumns: SheetColumn[] = useMemo(
    () => [
      {
        id: "code",
        header: "사원 번호",
        role: "linked-source",
        width: 100,
        type: "text",
        description: "인사기록 카드 고유 식별자 (Object Identity)",
        provenance: "Acme Group HCM Person Identity 원장",
        getValue: (r: Employee) => r.code,
        format: (val) => val,
        isEditable: false,
      },
      {
        id: "name",
        header: "성명",
        role: "linked-source",
        width: 110,
        type: "text",
        description: "법정 인적사항 실명",
        provenance: "국세청 소득세법 원천징수 대상자 명부",
        getValue: (r: Employee) => r.name,
        format: (val) => val,
        isEditable: false,
      },
      {
        id: "dept",
        header: "소속 부서",
        role: "object-reference",
        width: 130,
        type: "reference",
        referenceKind: "department",
        description: "조직도 발령 부서 포인터",
        provenance: "조직 개편 및 인사명령(PA) 발령 대장",
        getValue: (r: Employee) => r.dept,
        format: (val) => val,
        isEditable: false,
      },
      {
        id: "baseSalary",
        header: "약정 기본급",
        role: "linked-source",
        width: 130,
        type: "money",
        description: "고용계약서에 명시된 월 통상 기본급",
        provenance: "근로기준법 제17조 표준근로계약서 (EC-* 원장)",
        getValue: (r: Employee) => r.baseSalary,
        format: (val) => `${Number(val || 0).toLocaleString()}원`,
        isEditable: false,
      },
      {
        id: "proposedAllowance",
        header: "제안 수당 (Allowance)",
        role: "business-input",
        width: 160,
        type: "money",
        description: "당월 기안용 직책/정액 수당 (시트 내 편집 및 일괄 붙여넣기 가능)",
        provenance: "2026-07 급여 기안 초안 (Proposed Draft Value)",
        getValue: (r: Employee) => {
          const prop = proposedChanges.get(`${r.id}:proposedAllowance`);
          return prop !== undefined ? prop.proposedValue : r.fixedAllow || 0;
        },
        format: (val) => `${Number(val || 0).toLocaleString()}원`,
        isEditable: true,
      },
      {
        id: "overtimeSummary",
        header: "실적 연장/야간 (h)",
        role: "linked-source",
        width: 120,
        type: "text",
        description: "전자 타임카드 확정 연장 및 야간 근로시간",
        provenance: "근태기록 및 지문인식기 출퇴근 원장",
        getValue: (r: Employee) => {
          const att = attendance.find((a) => a.employeeId === r.id);
          const ot = att?.otHours || 0;
          const night = att?.nightHours || 0;
          return `연장 ${ot}h / 야간 ${night}h`;
        },
        format: (val) => val,
        isEditable: false,
      },
      {
        id: "calculatedGross",
        header: "산출 지급총액 (Gross)",
        role: "calculated-result",
        width: 160,
        type: "money",
        description: "기본급 + 제안수당 + 법정가산수당 소유자 계산 결과",
        provenance: "Payroll Kernel §56 가산수당 실시간 계산식",
        getValue: (r: Employee) => {
          const prop = proposedChanges.get(`${r.id}:proposedAllowance`);
          const allow = prop !== undefined ? Number(prop.proposedValue) : Number(r.fixedAllow || 0);
          const att = attendance.find((a) => a.employeeId === r.id);
          const otHours = att?.otHours || 0;
          const nightHours = att?.nightHours || 0;
          const hBase = (Number(r.baseSalary) + allow) / 209;
          const otPay = Math.round(otHours * hBase * 1.5);
          const nightPay = Math.round(nightHours * hBase * 0.5);
          return Number(r.baseSalary) + allow + otPay + nightPay;
        },
        format: (val) => `${Number(val || 0).toLocaleString()}원`,
        isEditable: false,
      },
    ],
    [attendance, proposedChanges]
  );

  const allColumns = useMemo(
    () => [...baseColumns, ...analyticalColumns],
    [baseColumns, analyticalColumns]
  );

  // Column letters (A, B, C...)
  const getColLetter = (index: number) => String.fromCharCode(65 + index);

  // Current active row and column
  const activeRow = useMemo(
    () => employees.find((e) => e.id === activeCell.rowKey) || employees[0],
    [employees, activeCell.rowKey]
  );
  const activeCol = useMemo(
    () => allColumns[activeCell.colIndex] || allColumns[0],
    [allColumns, activeCell.colIndex]
  );

  // Update formula bar value when active cell changes
  useEffect(() => {
    if (!activeRow || !activeCol) return;
    const currentVal = activeCol.getValue(activeRow);
    setFormulaBarValue(activeCol.formula ? activeCol.formula : String(currentVal ?? ""));
  }, [activeCell, activeRow, activeCol]);

  // Focus input when entering edit mode
  useEffect(() => {
    if (isEditing && cellInputRef.current) {
      cellInputRef.current.focus();
      cellInputRef.current.select();
    }
  }, [isEditing]);

  // Handle cell navigation
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (isEditing) {
      if (e.key === "Enter") {
        if (e.nativeEvent.isComposing || (e as any).keyCode === 229) {
          return;
        }
        e.preventDefault();
        handleCommitCellEdit(editValue);
        // Move down after Enter
        moveActiveCell(1, 0);
      } else if (e.key === "Escape") {
        e.preventDefault();
        setIsEditing(false);
      } else if (e.key === "Tab") {
        e.preventDefault();
        handleCommitCellEdit(editValue);
        moveActiveCell(0, e.shiftKey ? -1 : 1);
      }
      return;
    }

    // Navigation keys when not editing
    switch (e.key) {
      case "ArrowUp":
        e.preventDefault();
        moveActiveCell(-1, 0);
        break;
      case "ArrowDown":
        e.preventDefault();
        moveActiveCell(1, 0);
        break;
      case "ArrowLeft":
        e.preventDefault();
        moveActiveCell(0, -1);
        break;
      case "ArrowRight":
        e.preventDefault();
        moveActiveCell(0, 1);
        break;
      case "Tab":
        e.preventDefault();
        moveActiveCell(0, e.shiftKey ? -1 : 1);
        break;
      case "Enter":
        e.preventDefault();
        if (activeCol.isEditable) {
          startCellEdit();
        } else {
          moveActiveCell(1, 0);
        }
        break;
      default:
        // If typing directly on an editable cell
        if (activeCol.isEditable && e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
          e.preventDefault();
          startCellEdit(e.key);
        }
        break;
    }
  };

  const moveActiveCell = (rowDelta: number, colDelta: number) => {
    const currentRowIdx = employees.findIndex((e) => e.id === activeCell.rowKey);
    const newRowIdx = Math.max(0, Math.min(employees.length - 1, currentRowIdx + rowDelta));
    const newColIdx = Math.max(0, Math.min(allColumns.length - 1, activeCell.colIndex + colDelta));
    const targetEmp = employees[newRowIdx];
    const targetCol = allColumns[newColIdx];

    if (targetEmp && targetCol) {
      setActiveCell({
        rowKey: targetEmp.id,
        colIndex: newColIdx,
        columnId: targetCol.id,
      });
      setIsEditing(false);
    }
  };

  const startCellEdit = (initialVal?: string) => {
    if (!activeCol.isEditable) return;
    const currentVal = activeCol.getValue(activeRow);
    setEditValue(initialVal !== undefined ? initialVal : String(currentVal ?? ""));
    setIsEditing(true);
  };

  const handleCommitCellEdit = (valString: string) => {
    if (!activeRow || !activeCol) return;
    let finalVal: any = valString;

    if (activeCol.type === "money" || activeCol.type === "number") {
      const parsed = parseMoneyInput(valString);
      finalVal = parsed !== null ? parsed : 0;
    }

    const prevVal = activeCol.getValue(activeRow);
    const changeKey = `${activeRow.id}:${activeCol.id}`;

    const newChange: ProposedChange = {
      rowKey: activeRow.id,
      columnId: activeCol.id,
      previousValue: prevVal,
      proposedValue: finalVal,
      status: "draft",
      timestamp: new Date().toISOString(),
    };

    setProposedChanges((prev) => {
      const next = new Map(prev);
      next.set(changeKey, newChange);
      return next;
    });

    setIsEditing(false);
  };

  // 6. First-class Multi-Cell TSV Paste Handler
  const handlePaste = (e: React.ClipboardEvent) => {
    e.preventDefault();
    const pasteText = e.clipboardData.getData("text");
    if (!pasteText) return;

    const rows = parseTsvClipboard(pasteText);
    if (rows.length === 0) return;

    const startRowIdx = employees.findIndex((e) => e.id === activeCell.rowKey);
    const startColIdx = activeCell.colIndex;

    let appliedCount = 0;
    const nextChanges = new Map(proposedChanges);

    rows.forEach((rowVals, rOffset) => {
      const targetRow = employees[startRowIdx + rOffset];
      if (!targetRow) return;

      rowVals.forEach((cellVal, cOffset) => {
        const targetCol = allColumns[startColIdx + cOffset];
        if (!targetCol || !targetCol.isEditable) return;

        let parsedVal: any = cellVal;
        if (targetCol.type === "money" || targetCol.type === "number") {
          const money = parseMoneyInput(cellVal);
          if (money !== null) parsedVal = money;
        }

        const prev = targetCol.getValue(targetRow);
        nextChanges.set(`${targetRow.id}:${targetCol.id}`, {
          rowKey: targetRow.id,
          columnId: targetCol.id,
          previousValue: prev,
          proposedValue: parsedVal,
          status: "draft",
          timestamp: new Date().toISOString(),
        });
        appliedCount++;
      });
    });

    setProposedChanges(nextChanges);
    setPasteToast(`${appliedCount}개 셀 클립보드 붙여넣기 적용 완료 (임시저장)`);
    setTimeout(() => setPasteToast(null), 3000);
  };

  // 7. Change Summary for Consequence Modal
  const summary = useMemo(
    () => summarizeProposedAllowanceChanges(proposedChanges, employees),
    [proposedChanges, employees]
  );

  const consequenceSummary: ConsequenceSummary = {
    title: "2026년 7월 급여 수당 변경안 승인 및 급여 원장 갱신",
    actionType: "수당기안확정",
    subject: `(주)오야티 코퍼레이션 임직원 ${summary.affectedHeadcount}명 대상 제안 수당 반영`,
    effectiveDate: "2026-07-01 (당월 급여 전액 소급 적용)",
    summaryLines: [
      `총 ${summary.changeCount}건의 제안 수당 변경 반영 (순 변동 총액: ${summary.totalAllowanceDelta > 0 ? "+" : ""}${summary.totalAllowanceDelta.toLocaleString()}원)`,
      "반영 즉시 법정 연장/야간 통상시급 및 4대보험, 소득세 원천징수액이 실시간 재산정됩니다.",
      "금융결제원(KFTC) 100-byte 펌뱅킹 대량 이체 전문에 본 변경분이 자동 연동됩니다.",
    ],
    historicalImpact: "본 변경 이력은 변경 전 수당과 변경 후 제안 수당 diff 대조표와 함께 감사 원장에 영구 보존됩니다.",
    requiresApproval: true,
    approverRole: "재무총괄 / CFO 대행",
    consequenceButtonLabel: "수당 변경안 확정 및 급여 원장 갱신",
  };

  const handleConfirmCommit = () => {
    const changesArray = Array.from(proposedChanges.values());
    onCommitChanges(changesArray);
    setProposedChanges(new Map());
    setConsequenceModalOpen(false);
  };

  // Add analytical scenario column
  const handleAddScenarioColumn = () => {
    const colId = `scenario_${Date.now()}`;
    const newCol: SheetColumn = {
      id: colId,
      header: "예산 인상 5% (시나리오)",
      role: "sheet-formula",
      width: 150,
      type: "formula",
      formula: "=[@calculatedGross] * 1.05",
      description: "당월 총지급액 5% 인상 시 가상 예산 비교",
      provenance: "사용자 정의 시나리오 공식 ([@calculatedGross] * 1.05)",
      getValue: (r) => {
        const baseCol = allColumns.find((c) => c.id === "calculatedGross");
        const val = baseCol ? Number(baseCol.getValue(r)) : 0;
        return Math.round(val * 1.05);
      },
      format: (val) => `${Number(val || 0).toLocaleString()}원`,
      isEditable: false,
    };
    setAnalyticalColumns((prev) => [...prev, newCol]);
  };

  return (
    <div
      ref={gridContainerRef}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onPaste={handlePaste}
      className="flex flex-col h-[calc(100vh-210px)] min-h-[540px] border border-[var(--border)] rounded-lg bg-[var(--surface)] shadow-2xs overflow-hidden focus:outline-none focus:ring-1 focus:ring-blue-400 select-none layout-contain-table"
    >
      {/* 1. 시트 탑 컨트롤 바 (Top Action Bar) */}
      <div className="flex flex-wrap items-center justify-between px-3 py-2 border-b border-[var(--border)] bg-gray-50/70 gap-2">
        <div className="flex items-center gap-2.5">
          <div className="flex items-center gap-1.5 font-bold text-xs text-[var(--ink)]">
            <FileSpreadsheet className="w-4 h-4 text-emerald-700" />
            <span>급여 기안 및 수당 산정 시트 (Connected Sheet)</span>
          </div>

          {/* 스냅샷 vs 실시간 모드 토글 */}
          <div className="flex items-center rounded-md border border-[var(--border)] bg-white p-0.5 text-[11px]">
            <button
              type="button"
              onClick={() => setSnapshotMode("live")}
              className={`flex items-center gap-1 px-2 py-0.5 rounded cursor-pointer transition-colors ${
                snapshotMode === "live"
                  ? "bg-emerald-50 text-emerald-800 font-bold border border-emerald-200"
                  : "text-[var(--steel)] hover:text-[var(--ink)]"
              }`}
            >
              <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse" />
              <span>실시간 연동 (Live)</span>
            </button>
            <button
              type="button"
              onClick={() => setSnapshotMode("snapshot")}
              className={`flex items-center gap-1 px-2 py-0.5 rounded cursor-pointer transition-colors ${
                snapshotMode === "snapshot"
                  ? "bg-blue-50 text-blue-800 font-bold border border-blue-200"
                  : "text-[var(--steel)] hover:text-[var(--ink)]"
              }`}
            >
              <Calendar className="w-3 h-3 text-blue-600" />
              <span>2026-07-28 스냅샷</span>
            </button>
          </div>

          {/* 초안 보존 상태 표시기 */}
          {proposedChanges.size > 0 ? (
            <span className="inline-flex items-center gap-1 text-[11px] font-mono text-amber-800 bg-amber-50 px-2 py-0.5 rounded border border-amber-300">
              <Edit3 className="w-3 h-3 text-amber-600" />
              <span>
                초안 보존됨 · 제안 변경 {proposedChanges.size}건 (
                {summary.totalAllowanceDelta >= 0 ? "+" : ""}
                {summary.totalAllowanceDelta.toLocaleString()}원)
              </span>
            </span>
          ) : (
            <span className="text-[11px] text-[var(--steel)] flex items-center gap-1">
              <CheckCircle2 className="w-3 h-3 text-emerald-600" />
              <span>원장 데이터와 동기화됨</span>
            </span>
          )}
        </div>

        {/* 우측 액션 버튼들 */}
        <div className="flex items-center gap-2">
          {pasteToast && (
            <span className="text-[11px] text-blue-700 bg-blue-50 px-2 py-1 rounded border border-blue-200 animate-in fade-in">
              {pasteToast}
            </span>
          )}
          <Button
            size="xs"
            variant="secondary"
            leftIcon={<Plus className="w-3 h-3" />}
            onClick={handleAddScenarioColumn}
          >
            분석 열 추가
          </Button>
          <Button
            size="xs"
            variant="secondary"
            leftIcon={<Save className="w-3 h-3" />}
            onClick={() => onSaveDraft?.(Array.from(proposedChanges.values()))}
          >
            초안 임시저장
          </Button>
          <Button
            size="xs"
            variant="brand"
            disabled={proposedChanges.size === 0}
            leftIcon={<Check className="w-3 h-3" />}
            onClick={() => setConsequenceModalOpen(true)}
          >
            제안 검토 및 급여안 확정
          </Button>
        </div>
      </div>

      {/* 2. 엑셀 표준 수식 입력줄 (Formula Bar) */}
      <div className="flex items-center px-3 py-1.5 border-b border-[var(--border)] bg-white text-xs gap-3">
        {/* 셀 좌표 표기 (e.g. E3) */}
        <div className="flex items-center gap-1.5 shrink-0 border-r border-[var(--border)] pr-3">
          <span className="w-8 text-center font-mono font-bold bg-gray-100 px-1 py-0.5 rounded text-[var(--ink)]">
            {getColLetter(activeCell.colIndex)}
            {employees.findIndex((e) => e.id === activeCell.rowKey) + 1}
          </span>
          {/* 셀 역할 배지 (Role Badge) */}
          <span
            className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${
              activeCol.role === "linked-source"
                ? "bg-purple-100 text-purple-800"
                : activeCol.role === "business-input"
                ? "bg-amber-100 text-amber-900 border border-amber-300 font-bold"
                : activeCol.role === "calculated-result"
                ? "bg-blue-100 text-blue-800"
                : activeCol.role === "sheet-formula"
                ? "bg-indigo-100 text-indigo-800"
                : "bg-teal-100 text-teal-800"
            }`}
          >
            {activeCol.role === "linked-source" && "연계 원천 (Linked Source)"}
            {activeCol.role === "business-input" && "비즈니스 입력 (Editable Input)"}
            {activeCol.role === "calculated-result" && "산출 결과 (Calculated Gross)"}
            {activeCol.role === "sheet-formula" && "시트 수식 (Formula)"}
            {activeCol.role === "analytical-input" && "분석 메모 (Planner Note)"}
            {activeCol.role === "object-reference" && "객체 참조 (Object Pointer)"}
          </span>
        </div>

        {/* fx 라벨 및 수식/값 입력칸 */}
        <div className="flex items-center gap-2 flex-1">
          <span className="font-serif italic font-bold text-[var(--steel)] text-xs">fx</span>
          <input
            type="text"
            value={formulaBarValue}
            onChange={(e) => setFormulaBarValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                if (e.nativeEvent.isComposing || (e as any).keyCode === 229) {
                  return;
                }
                handleCommitCellEdit(formulaBarValue);
              }
            }}
            readOnly={!activeCol.isEditable}
            placeholder={activeCol.isEditable ? "값 입력 또는 Excel 수식..." : "읽기 전용 필드"}
            className="w-full bg-transparent font-mono text-xs text-[var(--ink)] focus:outline-none"
          />
        </div>

        {/* 셀 인스펙터 버튼 */}
        <button
          type="button"
          onClick={() => setCellInspectorOpen(!cellInspectorOpen)}
          className="text-[var(--steel)] hover:text-[var(--ink)] cursor-pointer text-[11px] flex items-center gap-1 shrink-0"
        >
          <Info className="w-3.5 h-3.5" />
          <span>셀 출처 및 연산 규칙</span>
        </button>
      </div>

      {/* 3. 그리드 바디 & 셀 인스펙터 스플릿 뷰 */}
      <div className="flex-1 flex overflow-hidden">
        {/* 스프레드시트 메인 테이블 */}
        <div className="flex-1 overflow-auto">
          <table className="w-full border-collapse text-xs font-sans">
            {/* 고정 컬럼 헤더 */}
            <thead className="sticky top-0 z-20 bg-gray-100 border-b border-[var(--border)] text-[var(--steel)] font-medium">
              <tr>
                {/* 행 번호 컬럼 헤더 (좌상단 빈 코너) */}
                <th className="w-10 px-2 py-1.5 text-center font-mono text-[10px] border-r border-[var(--border)] bg-gray-200/60 sticky left-0 z-30">
                  #
                </th>
                {allColumns.map((col, idx) => (
                  <th
                    key={col.id}
                    style={{ width: col.width, minWidth: col.width }}
                    className="px-2.5 py-1.5 text-left font-semibold border-r border-[var(--border)] group select-none"
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-1.5">
                        <span className="font-mono text-[10px] text-gray-400">
                          {getColLetter(idx)}
                        </span>
                        <span className="text-[var(--ink)] truncate">{col.header}</span>
                      </div>
                      {col.role === "linked-source" && (
                        <span title="원천 객체 연계 (읽기 전용)">
                          <Lock className="w-2.5 h-2.5 text-gray-400" />
                        </span>
                      )}
                      {col.role === "business-input" && (
                        <span title="기안 초안 편집 가능">
                          <Edit3 className="w-2.5 h-2.5 text-amber-600 font-bold" />
                        </span>
                      )}
                      {col.role === "calculated-result" && (
                        <span title="소유자 산출식 자동 계산">
                          <Calculator className="w-2.5 h-2.5 text-blue-600" />
                        </span>
                      )}
                      {col.role === "sheet-formula" && (
                        <span title="시트 수식 열">
                          <Sparkles className="w-2.5 h-2.5 text-indigo-600" />
                        </span>
                      )}
                    </div>
                  </th>
                ))}
              </tr>
            </thead>

            {/* 행 바디 */}
            <tbody className="divide-y divide-[var(--border-soft)] bg-white font-mono">
              {employees.map((emp, rowIdx) => {
                const isSelectedRow = activeCell.rowKey === emp.id;
                return (
                  <tr
                    key={emp.id}
                    className={`hover:bg-blue-50/30 transition-colors ${
                      isSelectedRow ? "bg-blue-50/20" : ""
                    }`}
                  >
                    {/* 고정 행 번호 (1, 2, 3...) */}
                    <td className="w-10 px-1 py-1.5 text-center text-[10px] text-gray-400 border-r border-[var(--border)] bg-gray-50/70 sticky left-0 z-10 select-none">
                      {rowIdx + 1}
                    </td>

                    {/* 각 셀 렌더링 */}
                    {allColumns.map((col, colIdx) => {
                      const isActive =
                        activeCell.rowKey === emp.id && activeCell.colIndex === colIdx;
                      const rawVal = col.getValue(emp);
                      const formattedVal = col.format ? col.format(rawVal) : String(rawVal ?? "");
                      const propChange = proposedChanges.get(`${emp.id}:${col.id}`);
                      const isModified = propChange !== undefined;

                      return (
                        <td
                          key={col.id}
                          onClick={() => {
                            setActiveCell({
                              rowKey: emp.id,
                              colIndex: colIdx,
                              columnId: col.id,
                            });
                            setIsEditing(false);
                            onSelectEmployee?.(emp);
                          }}
                          onDoubleClick={() => {
                            if (col.isEditable) startCellEdit();
                          }}
                          className={`px-2.5 py-1.5 border-r border-[var(--border-soft)] text-xs relative cursor-default transition-all ${
                            isActive
                              ? "ring-2 ring-blue-600 ring-inset bg-blue-50/50 z-10 font-bold"
                              : ""
                          } ${isModified ? "bg-amber-50/60 font-semibold" : ""}`}
                        >
                          {/* 편집 모드 인라인 인풋 */}
                          {isActive && isEditing && col.isEditable ? (
                            <input
                              ref={cellInputRef}
                              type="text"
                              value={editValue}
                              onChange={(e) => setEditValue(e.target.value)}
                              onBlur={() => handleCommitCellEdit(editValue)}
                              className="w-full h-full bg-white border border-blue-500 px-1 py-0.5 text-xs font-mono text-[var(--ink)] focus:outline-none shadow-inner"
                            />
                          ) : (
                            <div className="flex items-center justify-between gap-1 truncate">
                              <span
                                className={`truncate ${
                                  col.type === "money"
                                    ? "text-right w-full font-mono"
                                    : col.role === "linked-source"
                                    ? "text-[var(--ink)]"
                                    : col.role === "calculated-result"
                                    ? "text-blue-900 font-bold"
                                    : "text-[var(--ink)]"
                                }`}
                              >
                                {formattedVal}
                              </span>

                              {/* 변경 제안 diff 표시기 */}
                              {isModified && col.role === "business-input" && (
                                <span className="text-[9px] font-mono font-bold text-amber-700 bg-amber-100 px-1 rounded shrink-0">
                                  DRAFT
                                </span>
                              )}
                            </div>
                          )}

                          {/* 활성 셀 우하단 Fill Handle 시각화 */}
                          {isActive && (
                            <div className="absolute right-0 bottom-0 w-2 h-2 bg-blue-600 cursor-crosshair" />
                          )}
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>

        {/* 4. 셀 출처 및 연산 규칙 인스펙터 패널 (Collapsible Inspector) */}
        {cellInspectorOpen && activeCol && activeRow && (
          <div className="w-80 border-l border-[var(--border)] bg-gray-50/80 p-4 space-y-4 overflow-y-auto animate-in slide-in-from-right duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-2">
              <div className="flex items-center gap-1.5">
                <Info className="w-4 h-4 text-blue-600" />
                <h4 className="text-xs font-bold text-[var(--ink)]">셀 인스펙터 (Provenance)</h4>
              </div>
              <button
                type="button"
                onClick={() => setCellInspectorOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer text-xs"
              >
                ✕
              </button>
            </div>

            {/* 기본 메타데이터 */}
            <div className="p-2.5 rounded-md border border-[var(--border)] bg-white space-y-1.5 text-xs">
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">셀 좌표:</span>
                <span className="font-mono font-bold text-blue-700">
                  {getColLetter(activeCell.colIndex)}
                  {employees.findIndex((e) => e.id === activeCell.rowKey) + 1}
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">대상 사원:</span>
                <span className="font-bold text-[var(--ink)]">
                  {activeRow.name} ({activeRow.code})
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">컬럼 명칭:</span>
                <span className="font-semibold text-[var(--ink)]">{activeCol.header}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">셀 역할 구분:</span>
                <span className="font-bold text-emerald-800">{activeCol.role}</span>
              </div>
            </div>

            {/* 법정/원천 출처 안내 */}
            <div className="space-y-1 text-xs">
              <span className="font-bold text-[var(--steel)] block text-[11px] uppercase tracking-wider">
                원천 객체 식별 및 거버넌스 출처:
              </span>
              <div className="p-2.5 rounded border border-blue-200 bg-blue-50/60 text-blue-950 text-[11px] leading-relaxed">
                {activeCol.provenance || "Acme Group Object Engine Governance"}
              </div>
            </div>

            {/* 산출 근거 공식 설명 */}
            <div className="space-y-1 text-xs">
              <span className="font-bold text-[var(--steel)] block text-[11px] uppercase tracking-wider">
                연산 및 비즈니스 규칙:
              </span>
              <div className="p-2.5 rounded border border-[var(--border)] bg-white text-[11px] text-[var(--steel)] leading-relaxed">
                {activeCol.description || "해당 필드는 고용계약 및 근로기준법에 의해 보호됩니다."}
              </div>
            </div>

            {/* 현재 값 & 제안 값 비교 */}
            <div className="p-2.5 rounded border border-[var(--border)] bg-white space-y-1 text-xs">
              <div className="flex justify-between">
                <span className="text-[var(--steel)]">현재 원장 값:</span>
                <span className="font-mono">{activeCol.format ? activeCol.format(activeCol.getValue(activeRow)) : String(activeCol.getValue(activeRow))}</span>
              </div>
              {proposedChanges.has(`${activeRow.id}:${activeCol.id}`) && (
                <div className="flex justify-between text-amber-800 font-bold border-t pt-1">
                  <span>제안 초안 값:</span>
                  <span className="font-mono">
                    {activeCol.format
                      ? activeCol.format(proposedChanges.get(`${activeRow.id}:${activeCol.id}`)?.proposedValue)
                      : String(proposedChanges.get(`${activeRow.id}:${activeCol.id}`)?.proposedValue)}
                  </span>
                </div>
              )}
            </div>
          </div>
        )}
      </div>

      {/* 5. 변경 사항 검토 및 급여안 확정 Consequence Modal */}
      <ConsequenceModal
        isOpen={consequenceModalOpen}
        onClose={() => setConsequenceModalOpen(false)}
        onConfirm={handleConfirmCommit}
        summary={consequenceSummary}
      />
    </div>
  );
}
