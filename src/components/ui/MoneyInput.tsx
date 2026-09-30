"use client";

import React, { useState, useEffect } from "react";
import { AlertCircle } from "lucide-react";

interface MoneyInputProps {
  label: string;
  value: number;
  onChange: (val: number) => void;
  required?: boolean;
  advisoryThreshold?: number; // e.g. 15,000,000 won
  advisoryMessage?: string;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}

export function MoneyInput({
  label,
  value,
  onChange,
  required = false,
  advisoryThreshold = 15000000,
  advisoryMessage = "일반 급여 밴드 상한을 초과하는 금액입니다. 임원 또는 특별 승인 건인지 확인하십시오.",
  placeholder = "0",
  disabled = false,
  className = "",
}: MoneyInputProps) {
  const [displayValue, setDisplayValue] = useState<string>(
    value ? value.toLocaleString("ko-KR") : ""
  );

  useEffect(() => {
    setDisplayValue(value ? value.toLocaleString("ko-KR") : "");
  }, [value]);

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    // 숫자 이외의 문자 제거
    const rawNumber = e.target.value.replace(/[^0-9]/g, "");
    if (!rawNumber) {
      setDisplayValue("");
      onChange(0);
      return;
    }

    const num = parseInt(rawNumber, 10);
    setDisplayValue(num.toLocaleString("ko-KR"));
    onChange(num);
  };

  const isAdvisoryWarning = value > advisoryThreshold;

  return (
    <div className={`space-y-1 ${className}`}>
      <label className="block text-xs font-semibold text-[var(--ink)]">
        {label} {required && <span className="text-red-500">*</span>}
      </label>

      <div className="relative rounded-md shadow-2xs">
        <div className="pointer-events-none absolute inset-y-0 left-0 flex items-center pl-3">
          <span className="text-xs font-bold text-[var(--steel)]">₩</span>
        </div>
        <input
          type="text"
          inputMode="numeric"
          disabled={disabled}
          value={displayValue}
          onChange={handleChange}
          placeholder={placeholder}
          className={`block w-full rounded-md border py-1.5 pl-8 pr-12 text-xs font-mono font-medium text-[var(--ink)] transition-colors focus:outline-none focus:ring-1 ${
            isAdvisoryWarning
              ? "border-amber-400 bg-amber-50/20 focus:border-amber-500 focus:ring-amber-400"
              : "border-[var(--border)] bg-[var(--surface)] focus:border-[var(--signal-deep)] focus:ring-[var(--signal)]"
          }`}
        />
        <div className="pointer-events-none absolute inset-y-0 right-0 flex items-center pr-3">
          <span className="text-[11px] text-[var(--steel)] font-mono">원</span>
        </div>
      </div>

      {/* Advisory Warning (비차단성 권고 안내) */}
      {isAdvisoryWarning && (
        <div className="flex items-center gap-1.5 text-[11px] text-amber-700 animate-in fade-in duration-150 pt-0.5">
          <AlertCircle className="w-3.5 h-3.5 shrink-0 text-amber-600" />
          <span>{advisoryMessage}</span>
        </div>
      )}
    </div>
  );
}
