// regulatory-registry.ts — Enterprise Statutory & Regulatory Evolution Engine
// Designed for 3-Year & 5-Year Regulatory Maintainability without Code Rewrites
// Handles past statutory schedules (2024, 2025), current production (2026),
// and projected future reforms (2027, 2028+) with effective-date range resolution.

import type { StatutoryRates } from "./payroll-engine";

export interface VersionedRegulatorySchedule {
  id: string;
  name: string;
  effectiveFrom: string; // YYYY-MM-DD
  effectiveTo: string;   // YYYY-MM-DD or "9999-12-31"
  status: "historical" | "active" | "proposed";
  rates: StatutoryRates;
  legalGazetteNotice?: string; // 관보 고시 번호 (예: 보건복지부 고시 제2025-112호)
  description: string;
}

/**
 * Versioned National Statutory Schedules
 */
export const STATUTORY_SCHEDULE_REGISTRY: VersionedRegulatorySchedule[] = [
  // -------------------------------------------------------------
  // 2024 Fiscal Year (Historical)
  // -------------------------------------------------------------
  {
    id: "REG-KR-2024",
    name: "2024년 대한민국 노동/사회보험 법정 요율",
    effectiveFrom: "2024-01-01",
    effectiveTo: "2024-12-31",
    status: "historical",
    legalGazetteNotice: "고용노동부 고시 제2023-44호",
    description: "2024년 최저임금 9,860원, 국민연금 상한 590만원, 건강보험 7.09%",
    rates: {
      version: "2024-KR",
      appliedFrom: "2024-01-01",
      npEmp: 4.5 / 100,
      npBandLow: 390000,
      npBandHigh: 5900000,
      hiTotal: 7.09 / 100,
      hiClampLow: 19780,
      hiClampHigh: 8481420,
      ltcNum: 9082,
      ltcDen: 70900,
      eiEmp: 0.9 / 100,
      otMult: 1.5,
      nightAdd: 0.5,
      holMult: 1.5,
      holOverMult: 2.0,
      minWage: 9860,
      stdMonthlyHours: 209,
    },
  },

  // -------------------------------------------------------------
  // 2025 Fiscal Year (Historical)
  // -------------------------------------------------------------
  {
    id: "REG-KR-2025",
    name: "2025년 대한민국 노동/사회보험 법정 요율",
    effectiveFrom: "2025-01-01",
    effectiveTo: "2025-12-31",
    status: "historical",
    legalGazetteNotice: "고용노동부 고시 제2024-61호",
    description: "2025년 최저임금 10,030원, 국민연금 상한 617만원, 건강보험 7.09%",
    rates: {
      version: "2025-KR",
      appliedFrom: "2025-01-01",
      npEmp: 4.5 / 100,
      npBandLow: 400000,
      npBandHigh: 6170000,
      hiTotal: 7.09 / 100,
      hiClampLow: 20160,
      hiClampHigh: 8876400,
      ltcNum: 9182,
      ltcDen: 70900,
      eiEmp: 0.9 / 100,
      otMult: 1.5,
      nightAdd: 0.5,
      holMult: 1.5,
      holOverMult: 2.0,
      minWage: 10030,
      stdMonthlyHours: 209,
    },
  },

  // -------------------------------------------------------------
  // 2026 Fiscal Year (Current Active Baseline)
  // -------------------------------------------------------------
  {
    id: "REG-KR-2026",
    name: "2026년 대한민국 노동/사회보험 법정 요율 (현행)",
    effectiveFrom: "2026-01-01",
    effectiveTo: "2026-12-31",
    status: "active",
    legalGazetteNotice: "고용노동부 고시 제2025-88호 / 보건복지부 고시 제2026-14호",
    description: "2026년 최저시급 10,320원, 국민연금 모수개혁 4.75%, 건강보험 7.19%",
    rates: {
      version: "2026-07 (KR)",
      appliedFrom: "2026-01-01",
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
    },
  },

  // -------------------------------------------------------------
  // 2027 Fiscal Year (Proposed / Forward Planning)
  // -------------------------------------------------------------
  {
    id: "REG-KR-2027-PROPOSED",
    name: "2027년 대한민국 사회보험 개정안 (모의 시뮬레이션용)",
    effectiveFrom: "2027-01-01",
    effectiveTo: "2027-12-31",
    status: "proposed",
    legalGazetteNotice: "국민연금 모수개혁 2단계 입법예고안",
    description: "2027년 예상 최저시급 10,750원, 국민연금 5.0%, 건강보험 7.35%",
    rates: {
      version: "2027-PROPOSED (KR)",
      appliedFrom: "2027-01-01",
      npEmp: 5.0 / 100,
      npBandLow: 430000,
      npBandHigh: 6980000,
      hiTotal: 7.35 / 100,
      hiClampLow: 22000,
      hiClampHigh: 9750000,
      ltcNum: 9850,
      ltcDen: 73500,
      eiEmp: 0.9 / 100,
      otMult: 1.5,
      nightAdd: 0.5,
      holMult: 1.5,
      holOverMult: 2.0,
      minWage: 10750,
      stdMonthlyHours: 209,
    },
  },

  // -------------------------------------------------------------
  // 2028+ Long-Range Horizon (Projected)
  // -------------------------------------------------------------
  {
    id: "REG-KR-2028-HORIZON",
    name: "2028년 장기 재정추계 시뮬레이션 요율",
    effectiveFrom: "2028-01-01",
    effectiveTo: "9999-12-31",
    status: "proposed",
    legalGazetteNotice: "중장기 재정전망 추계치",
    description: "2028년 이후 예상 최저시급 11,200원, 국민연금 5.25%, 건강보험 7.50%",
    rates: {
      version: "2028-HORIZON (KR)",
      appliedFrom: "2028-01-01",
      npEmp: 5.25 / 100,
      npBandLow: 450000,
      npBandHigh: 7300000,
      hiTotal: 7.50 / 100,
      hiClampLow: 23500,
      hiClampHigh: 10200000,
      ltcNum: 10100,
      ltcDen: 75000,
      eiEmp: 0.9 / 100,
      otMult: 1.5,
      nightAdd: 0.5,
      holMult: 1.5,
      holOverMult: 2.0,
      minWage: 11200,
      stdMonthlyHours: 209,
    },
  },
];

/**
 * Resolves the statutory rate schedule effective for any given date or YYYY-MM
 * Guarantees retrospective accuracy for past recalculations and seamless forward execution for new fiscal years.
 */
export function resolveStatutoryRates(targetDateOrYearMonth?: string): StatutoryRates {
  if (!targetDateOrYearMonth) {
    // Default to current active schedule
    const active = STATUTORY_SCHEDULE_REGISTRY.find((s) => s.status === "active");
    return active ? active.rates : STATUTORY_SCHEDULE_REGISTRY[2].rates;
  }

  // Format standardization: "2024-05" -> "2024-05-15"
  const normalized = targetDateOrYearMonth.length === 7 ? `${targetDateOrYearMonth}-15` : targetDateOrYearMonth;

  for (const schedule of STATUTORY_SCHEDULE_REGISTRY) {
    if (normalized >= schedule.effectiveFrom && normalized <= schedule.effectiveTo) {
      return schedule.rates;
    }
  }

  // Fallback if before earliest or after latest
  if (normalized < STATUTORY_SCHEDULE_REGISTRY[0].effectiveFrom) {
    return STATUTORY_SCHEDULE_REGISTRY[0].rates;
  }
  return STATUTORY_SCHEDULE_REGISTRY[STATUTORY_SCHEDULE_REGISTRY.length - 1].rates;
}

/**
 * Returns all registered schedules for administrative governance inspection in /org/policy
 */
export function getRegisteredStatutorySchedules(): VersionedRegulatorySchedule[] {
  return STATUTORY_SCHEDULE_REGISTRY;
}
