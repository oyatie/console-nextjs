// attendance-calculator.ts — 근로기준법 제54조(휴게) 및 제53조(주52시간) 준용 계산 엔진

export interface TimePunchResult {
  grossHours: number | null;
  breakMinutes: number;
  netWorkHours: number | null;
  isMidnightShift: boolean;
  isValid: boolean;
  errorReason?: string;
}

/**
 * 근로기준법 제54조:
 * 근로시간이 4시간인 경우에는 30분 이상, 8시간인 경우에는 1시간 이상의 휴게시간을 부여하여야 함.
 * 명시 휴게시간(br)이 있으면 그 값을 적용하고, 없으면 법정 기준 자동 산출.
 */
export function calculateWorkHours(
  clockIn: string | null,
  clockOut: string | null,
  explicitBreakMinutes: string | number | null = null
): TimePunchResult {
  if (!clockIn || !clockOut) {
    return {
      grossHours: null,
      breakMinutes: 0,
      netWorkHours: null,
      isMidnightShift: false,
      isValid: false,
      errorReason: !clockIn ? "출근 미기록" : "퇴근 미기록",
    };
  }

  const timeRegex = /^([01]\d|2[0-3]):([0-5]\d)$/;
  if (!timeRegex.test(clockIn) || !timeRegex.test(clockOut)) {
    return {
      grossHours: null,
      breakMinutes: 0,
      netWorkHours: null,
      isMidnightShift: false,
      isValid: false,
      errorReason: "시간 형식 오류 (HH:mm)",
    };
  }

  const [inH, inM] = clockIn.split(":").map(Number);
  const [outH, outM] = clockOut.split(":").map(Number);

  let inMinutes = inH * 60 + inM;
  let outMinutes = outH * 60 + outM;

  let isMidnightShift = false;
  if (outMinutes < inMinutes) {
    // 자정을 넘긴 야간/익일 퇴근
    outMinutes += 24 * 60;
    isMidnightShift = true;
  }

  const grossMinutes = outMinutes - inMinutes;
  const grossHours = grossMinutes / 60;

  if (grossMinutes === 0) {
    return {
      grossHours: 0,
      breakMinutes: 0,
      netWorkHours: 0,
      isMidnightShift: false,
      isValid: true,
    };
  }

  let breakMinutes = 0;
  if (explicitBreakMinutes !== null && explicitBreakMinutes !== "" && !isNaN(Number(explicitBreakMinutes))) {
    const rawBreak = Number(explicitBreakMinutes);
    // Security & Adversarial Hardening: 휴게시간은 음수일 수 없으며 총 체류시간(grossMinutes)을 초과할 수 없음
    breakMinutes = Math.max(0, Math.min(rawBreak, grossMinutes));
  } else {
    // 법정 자동 차감 (근로기준법 제54조)
    if (grossHours >= 8) {
      breakMinutes = 60;
    } else if (grossHours >= 4) {
      breakMinutes = 30;
    } else {
      breakMinutes = 0;
    }
  }

  const netMinutes = Math.max(0, grossMinutes - breakMinutes);
  const netWorkHours = Math.round((netMinutes / 60) * 10) / 10;

  return {
    grossHours: Math.round(grossHours * 10) / 10,
    breakMinutes,
    netWorkHours,
    isMidnightShift,
    isValid: true,
  };
}

export type Work52Status = "NORMAL" | "OVERTIME" | "WARNING" | "VIOLATION";

/**
 * 주 52시간제 (기본 40시간 + 연장 한도 12시간) 준수 여부 판정
 */
export function evaluate52HourCompliance(weeklyTotalHours: number): {
  status: Work52Status;
  label: string;
  tone: "ok" | "info" | "warn" | "danger";
  remainingHours: number;
} {
  const limit = 52.0;
  const remainingHours = Math.max(0, Math.round((limit - weeklyTotalHours) * 10) / 10);

  if (weeklyTotalHours > 52.0) {
    return {
      status: "VIOLATION",
      label: "주52h 초과 위반",
      tone: "danger",
      remainingHours: 0,
    };
  }
  if (weeklyTotalHours >= 48.0) {
    return {
      status: "WARNING",
      label: "한도 임박 (경고)",
      tone: "warn",
      remainingHours,
    };
  }
  if (weeklyTotalHours > 40.0) {
    return {
      status: "OVERTIME",
      label: "연장 근로중",
      tone: "info",
      remainingHours,
    };
  }
  return {
    status: "NORMAL",
    label: "정상 범위",
    tone: "ok",
    remainingHours,
  };
}
