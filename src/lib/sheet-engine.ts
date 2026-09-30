// sheet-engine.ts — Enterprise Connected Spreadsheet Engine
// Implements cell role semantics, formula parsing, rectangular TSV clipboard handling,
// disambiguation of object references, and proposed draft change calculation.

import { SheetColumn, ProposedChange } from "./types";

/**
 * 통화/금액 문자열 파서
 * "1,250,000", "₩1,250,000", "1250000원", "125만", "-50,000" 등 지원
 */
export function parseMoneyInput(raw: string): number | null {
  if (!raw || typeof raw !== "string") return null;
  const cleaned = raw.trim().replace(/[₩,\s원]/g, "");
  if (cleaned === "") return null;

  // "125만" 형태 처리
  if (cleaned.endsWith("만")) {
    const num = parseFloat(cleaned.slice(0, -1));
    return isNaN(num) ? null : Math.round(num * 10000);
  }

  const num = Number(cleaned);
  return isNaN(num) ? null : Math.round(num);
}

/**
 * 일자 파서: YYYY-MM-DD 표준화
 */
export function parseDateInput(raw: string): string | null {
  if (!raw) return null;
  const trimmed = raw.trim().replace(/\./g, "-").replace(/\//g, "-");
  const match = trimmed.match(/^(\d{4})-(\d{1,2})-(\d{1,2})$/);
  if (match) {
    const y = match[1];
    const m = match[2].padStart(2, "0");
    const d = match[3].padStart(2, "0");
    return `${y}-${m}-${d}`;
  }
  return null;
}

/**
 * 클립보드 TSV 파서 (Excel / Google Sheets / Numbers 복사 붙여넣기 완벽 호환)
 */
export function parseTsvClipboard(text: string): string[][] {
  if (!text) return [];
  // 줄바꿈 정규화 (\r\n -> \n)
  const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  // 마지막 빈 줄 제거
  const lines = normalized.endsWith("\n") ? normalized.slice(0, -1).split("\n") : normalized.split("\n");
  return lines.map((line) => line.split("\t").map((cell) => cell.trim()));
}

/**
 * 안전한 클라이언트 수식 평가기 (Safe Sheet Formula Evaluator)
 * 지원: =[@calculatedGross] - [@baseSalary], =[@proposedAllowance] * 1.05, 사칙연산
 */
export function evaluateSheetFormula(
  formula: string,
  rowRecord: Record<string, any> = {}
): number | string {
  if (!formula || typeof formula !== "string" || !formula.startsWith("=")) return formula;
  let expr = formula.slice(1).trim();

  // If rowRecord has a .row property (common context pattern), merge
  const record = rowRecord.row ? { ...rowRecord.row, ...rowRecord } : rowRecord;

  // [ @fieldName ] 또는 [@fieldName] 또는 [fieldName] 토큰을 해당 행의 실제 값으로 치환
  expr = expr.replace(/\[@?([a-zA-Z0-9_]+)\]/g, (_, fieldName) => {
    let val = record[fieldName];
    if (val === undefined && Array.isArray(record.columns)) {
      const col = record.columns.find((c: any) => c.id === fieldName);
      if (col && typeof col.getValue === "function") {
        val = col.getValue(record.row || record);
      }
    }
    if (val === undefined || val === null) return "0";
    if (typeof val === "number") return String(val);
    const parsed = parseMoneyInput(String(val));
    return parsed !== null ? String(parsed) : "0";
  });

  // SUM(...) 함수 지원
  expr = expr.replace(/SUM\s*\(([^)]+)\)/gi, (_, argsStr) => {
    const parts = argsStr.split(",").map((s: string) => s.trim());
    return `(${parts.join(" + ")})`;
  });

  // AVERAGE(...) 함수 지원
  expr = expr.replace(/AVERAGE\s*\(([^)]+)\)/gi, (_, argsStr) => {
    const parts = argsStr.split(",").map((s: string) => s.trim());
    return parts.length > 0 ? `((${parts.join(" + ")}) / ${parts.length})` : "0";
  });

  // MAX(...) 함수 지원
  expr = expr.replace(/MAX\s*\(([^)]+)\)/gi, (_, argsStr) => {
    return `Math.max(${argsStr})`;
  });

  // MIN(...) 함수 지원
  expr = expr.replace(/MIN\s*\(([^)]+)\)/gi, (_, argsStr) => {
    return `Math.min(${argsStr})`;
  });

  // IF(cond, trueVal, falseVal) 지원
  expr = expr.replace(/IF\s*\(([^,]+),([^,]+),([^)]+)\)/gi, (_, cond, trueVal, falseVal) => {
    return `((${cond.trim()}) ? (${trueVal.trim()}) : (${falseVal.trim()}))`;
  });

  // 안전 검사: 수식 허용 문자
  if (!/^[0-9+\-*/().\s,><=!?:]+$|Math\.(max|min)/.test(expr)) {
    return "#ERR:INVALID_CHAR";
  }

  try {
    // 안전한 수식 연산 함수
    // eslint-disable-next-line no-new-func
    const result = Function(`"use strict"; return (${expr});`)();
    if (typeof result === "number") {
      return isNaN(result) || !isFinite(result) ? "#DIV/0!" : Math.round(result);
    }
    return String(result);
  } catch {
    return "#VALUE!";
  }
}

/**
 * 객체 참조 모호성 분석 (Object Reference Disambiguation)
 * 예: "운영팀" 붙여넣기 시 본사 운영팀과 부산 현장 운영팀이 경합할 때 안내
 */
export interface DisambiguationCandidate {
  id: string;
  name: string;
  context: string;
}

export function matchObjectReference(
  input: string,
  candidates: DisambiguationCandidate[]
): {
  matchedId?: string;
  matchedName?: string;
  isAmbiguous: boolean;
  competingCandidates: DisambiguationCandidate[];
} {
  const query = input.trim().toLowerCase();
  const exactMatches = candidates.filter(
    (c) => c.name.toLowerCase() === query || c.id.toLowerCase() === query
  );

  if (exactMatches.length === 1) {
    return {
      matchedId: exactMatches[0].id,
      matchedName: exactMatches[0].name,
      isAmbiguous: false,
      competingCandidates: [],
    };
  }

  if (exactMatches.length > 1) {
    return {
      isAmbiguous: true,
      competingCandidates: exactMatches,
    };
  }

  // 부분 일치 탐색
  const partialMatches = candidates.filter((c) =>
    c.name.toLowerCase().includes(query)
  );

  if (partialMatches.length === 1) {
    return {
      matchedId: partialMatches[0].id,
      matchedName: partialMatches[0].name,
      isAmbiguous: false,
      competingCandidates: [],
    };
  }

  return {
    isAmbiguous: partialMatches.length > 1,
    competingCandidates: partialMatches,
  };
}

/**
 * 제안 변경 집계 및 재정적 파급효과 산출
 */
export interface ProposedChangesSummary {
  changeCount: number;
  affectedHeadcount: number;
  totalAllowanceDelta: number;
  totalProposedGrossDelta: number;
  items: {
    employeeId: string;
    employeeName: string;
    field: string;
    prevVal: number;
    newVal: number;
    diff: number;
  }[];
}

export function summarizeProposedAllowanceChanges(
  changes: Map<string, ProposedChange>,
  employees: { id: string; name: string; fixedAllow?: number }[]
): ProposedChangesSummary {
  const items: ProposedChangesSummary["items"] = [];
  const affectedEmpIds = new Set<string>();
  let totalDiff = 0;

  for (const [key, change] of changes.entries()) {
    if (change.columnId === "proposedAllowance" && change.status === "draft") {
      const emp = employees.find((e) => e.id === change.rowKey);
      if (emp) {
        const prev = typeof change.previousValue === "number" ? change.previousValue : (emp.fixedAllow || 0);
        const next = typeof change.proposedValue === "number" ? change.proposedValue : prev;
        const diff = next - prev;
        items.push({
          employeeId: emp.id,
          employeeName: emp.name,
          field: "직책 및 정액 수당",
          prevVal: prev,
          newVal: next,
          diff,
        });
        affectedEmpIds.add(emp.id);
        totalDiff += diff;
      }
    }
  }

  return {
    changeCount: items.length,
    affectedHeadcount: affectedEmpIds.size,
    totalAllowanceDelta: totalDiff,
    totalProposedGrossDelta: totalDiff,
    items,
  };
}
