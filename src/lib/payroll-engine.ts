// payroll-engine.ts — 한국 법정 급여 계산 순수 모듈 (Rust console-payroll-domain 정렬)
// 근로기준법 제56조 (가산수당), 국민연금법, 국민건강보험법, 고용보험법 준용

import { Employee, PayDeduction, Payslip } from "./types";
import { resolveStatutoryRates } from "./regulatory-registry";

export interface StatutoryRates {
  version: string;
  appliedFrom: string;
  npEmp: number;        // 국민연금 직원 부담율 (4.75%)
  npBandLow: number;    // 국민연금 하한 (410,000원)
  npBandHigh: number;   // 국민연금 상한 (6,590,000원)
  hiTotal: number;      // 건강보험 총 요율 (7.19%)
  hiClampLow: number;   // 최저보수월액 하한
  hiClampHigh: number;  // 최고보수월액 상한
  ltcNum: number;       // 장기요양보험 분자 (9448)
  ltcDen: number;       // 장기요양보험 분모 (71900)
  eiEmp: number;        // 고용보험 근로자 요율 (0.9%)
  otMult: number;       // 연장근로 가산배수 (1.5)
  nightAdd: number;     // 야간근로 가산 (0.5)
  holMult: number;      // 휴일근로 배수 (1.5)
  holOverMult: number;  // 8시간 초과 휴일근로 배수 (2.0)
  minWage: number;      // 최저시급 (2026년 기준 10,320원)
  stdMonthlyHours: number; // 월 통상근로시간수 (209시간)
}

export const DEFAULT_RATES: StatutoryRates = {
  version: "2026-07 (KR)",
  appliedFrom: "2026-07-01",
  npEmp: 4.75 / 100,
  npBandLow: 410000,
  npBandHigh: 6590000,
  hiTotal: 7.19 / 100,
  hiClampLow: 20160,
  hiClampHigh: 9183480,
  ltcNum: 9448,
  ltcDen: 71900,
  eiEmp: 0.9 / 100,
  otMult: 1.5,
  nightAdd: 0.5,
  holMult: 1.5,
  holOverMult: 2.0,
  minWage: 10320,
  stdMonthlyHours: 209,
};

const R = Math.round;

/** 국고금관리법 제47조 제1항 준용: 10원 미만 절사 */
export function trunc10(n: number): number {
  return Math.floor(n / 10) * 10;
}

/** 국민연금 기준소득월액: 1,000원 미만 절사 + 상·하한 밴드 클램핑 */
export function pensionBase(m: number, rates: StatutoryRates = DEFAULT_RATES): number {
  if (m < rates.npBandLow) return rates.npBandLow;
  if (m > rates.npBandHigh) return rates.npBandHigh;
  return m - (m % 1000);
}

/** 국세청 간이세액표 기반 근사 계산 (부양가족 수 반영) */
export function incomeTaxSimple(taxable: number, dependents: number = 1): number {
  const b = taxable - 1500000 - Math.max(0, dependents - 1) * 150000;
  if (b <= 0) return 0;
  if (b <= 1500000) return R(b * 0.04);
  if (b <= 4000000) return R(60000 + (b - 1500000) * 0.12);
  return R(360000 + (b - 4000000) * 0.20);
}

/** 4대보험 및 세금 법정 공제 산정 */
export function statutoryDeductions(
  taxable: number,
  dependents: number = 1,
  rates: StatutoryRates = DEFAULT_RATES
): PayDeduction[] {
  // 국민연금
  const np = trunc10(pensionBase(taxable, rates) * rates.npEmp);

  // 건강보험 (총액 산정 후 50% 및 국고금관리법 제47조 10원 절사)
  const hiTotalAmt = Math.min(Math.max(trunc10(taxable * rates.hiTotal), rates.hiClampLow), rates.hiClampHigh);
  const hi = trunc10(hiTotalAmt / 2);

  // 노인장기요양보험 (건강보험료액의 9448/71900 의 50% 및 10원 절사)
  const ltcTotalAmt = trunc10((hiTotalAmt * rates.ltcNum) / rates.ltcDen);
  const ltc = trunc10(ltcTotalAmt / 2);

  // 고용보험 (0.9% 및 10원 절사)
  const ei = trunc10(taxable * rates.eiEmp);

  // 소득세 및 지방소득세 (소득세의 10% 및 10원 절사)
  const tax = trunc10(incomeTaxSimple(taxable, dependents));
  const local = trunc10(tax * 0.1);

  return [
    { code: "np", label: "국민연금", amt: np, basis: `기준월액 ${pensionBase(taxable, rates).toLocaleString()}원 × 4.75% · 10원절사` },
    { code: "hi", label: "건강보험", amt: hi, basis: `총 ${hiTotalAmt.toLocaleString()}원 (7.19%) 의 50% · 10원절사` },
    { code: "ltc", label: "장기요양", amt: ltc, basis: `건보료액 × 9,448/71,900 의 50% · 10원절사` },
    { code: "ei", label: "고용보험", amt: ei, basis: `과세총액 × 0.9% · 10원절사` },
    { code: "tax", label: "소득세", amt: tax, basis: `간이세액표 근사 (부양가족 ${dependents}인) · 10원절사` },
    { code: "local", label: "지방소득세", amt: local, basis: `소득세액 × 10% · 10원절사` },
  ];
}

/** 통상시급 계산 */
export function hourlyBase(employee: Employee, rates: StatutoryRates = DEFAULT_RATES): number {
  if (employee.empType === "정규" || employee.empType === "계약" || employee.empType === "임원") {
    return (Number(employee.baseSalary) + Number(employee.fixedAllow || 0)) / rates.stdMonthlyHours;
  }
  if (employee.empType === "시급" || employee.empType === "파견") {
    return Number(employee.baseSalary);
  }
  if (employee.empType === "일당") {
    return Number(employee.baseSalary) / 8;
  }
  return 0;
}

export interface AttendanceHoursSummary {
  otHours: number;
  nightHours: number;
  holidayHours: number;
}

/** 근로자 1인 월 급여명세서 실시간 계산 (과거 소급/미래 법정 요율 자동 해결 지원) */
export function computeEmployeePayslip(
  employee: Employee,
  attendance: AttendanceHoursSummary,
  ratesOrYearMonth: StatutoryRates | string = DEFAULT_RATES,
  customYearMonth?: string
): Payslip {
  const rates: StatutoryRates =
    typeof ratesOrYearMonth === "string"
      ? resolveStatutoryRates(ratesOrYearMonth)
      : ratesOrYearMonth;
  const effectiveYearMonth =
    typeof ratesOrYearMonth === "string"
      ? ratesOrYearMonth
      : customYearMonth || "2026-07";

  const hBase = hourlyBase(employee, rates);

  // 법정 가산수당 (근로기준법 제56조 및 국고금관리법 제47조 제1항 10원 절사)
  const overtimePay = trunc10(attendance.otHours * hBase * rates.otMult);
  const nightPay = trunc10(attendance.nightHours * hBase * rates.nightAdd);
  const holidayPay = trunc10(attendance.holidayHours * hBase * rates.holMult);

  const basePay = employee.baseSalary;
  const fixedAllowance = employee.fixedAllow || 0;
  const grossPay = basePay + fixedAllowance + overtimePay + nightPay + holidayPay;

  const deductions = statutoryDeductions(grossPay, employee.dependents, rates);
  const totalDeductions = deductions.reduce((acc, cur) => acc + cur.amt, 0);
  const netPay = trunc10(grossPay - totalDeductions);

  return {
    id: `ps_${employee.id}_${Date.now()}`,
    code: `PS-${employee.code.replace("EMP-", "")}`,
    employeeId: employee.id,
    employeeName: employee.name,
    yearMonth: effectiveYearMonth,
    empType: employee.empType,
    basePay,
    fixedAllowance,
    overtimePay,
    nightPay,
    holidayPay,
    grossPay,
    deductions,
    totalDeductions,
    netPay,
  };
}
