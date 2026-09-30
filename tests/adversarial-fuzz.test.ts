import { describe, it, expect } from "vitest";
import { evaluate52HourCompliance, calculateWorkHours } from "../src/lib/attendance-calculator";
import { evaluateSheetFormula, parseTsvClipboard, parseMoneyInput } from "../src/lib/sheet-engine";
import { computeEmployeePayslip } from "../src/lib/payroll-engine";
import { resolveStatutoryRates } from "../src/lib/regulatory-registry";
import {
  calculateEventHash,
  appendDynamicAuditEvent,
  verifyAuditChainIntegrity,
} from "../src/lib/dynamic-factory";
import { AuditEvent, Employee } from "../src/lib/types";

describe("Adversarial Hardening, Mutations & Fuzzing Suite", () => {
  describe("1. Attendance Boundary & 52-Hour Compliance Fuzzing (LSA §50/§53)", () => {
    it("handles boundary hours cleanly without throwing or crashing", () => {
      const boundaryHours = [0, 20, 39.9, 40, 47.9, 48, 51.9, 52, 52.01, 60, 100, 168];

      boundaryHours.forEach((hours) => {
        const result = evaluate52HourCompliance(hours);
        expect(result).toBeDefined();
        expect(["NORMAL", "OVERTIME", "WARNING", "VIOLATION"]).toContain(result.status);

        if (hours <= 40) {
          expect(result.status).toBe("NORMAL");
          expect(result.tone).toBe("ok");
        } else if (hours < 48) {
          expect(result.status).toBe("OVERTIME");
          expect(result.tone).toBe("info");
        } else if (hours <= 52) {
          expect(result.status).toBe("WARNING");
          expect(result.tone).toBe("warn");
        } else {
          expect(result.status).toBe("VIOLATION");
          expect(result.tone).toBe("danger");
        }
      });
    });

    it("calculates work hours with statutory lunch/rest deductions correctly across midnight shifts", () => {
      // 09:00 to 18:00 (9h elapsed, 1h break = 8h worked)
      const normalDay = calculateWorkHours("09:00", "18:00", 60);
      expect(normalDay.netWorkHours).toBe(8);
      expect(normalDay.breakMinutes).toBe(60);

      // Night shift crossing midnight: 20:00 to 05:00 (9h elapsed, 1h break = 8h worked, isMidnightShift true)
      const nightShift = calculateWorkHours("20:00", "05:00", 60);
      expect(nightShift.netWorkHours).toBe(8);
      expect(nightShift.isMidnightShift).toBe(true);

      // Edge case: missing punch (null clockIn or clockOut)
      const missingPunch = calculateWorkHours(null, "18:00", 60);
      expect(missingPunch.netWorkHours).toBe(null);
      expect(missingPunch.isValid).toBe(false);
    });
  });

  describe("2. ConnectedSheet Formula Engine Adversarial Fuzzing", () => {
    const mockContext = {
      row: { baseSalary: 3000000, fixedAllow: 300000, otHours: 10 },
      columns: [
        { id: "baseSalary", getValue: (r: any) => r.baseSalary },
        { id: "fixedAllow", getValue: (r: any) => r.fixedAllow },
        { id: "otHours", getValue: (r: any) => r.otHours },
      ],
    };

    it("evaluates standard statistical and arithmetic formulas correctly", () => {
      expect(evaluateSheetFormula("=SUM(100, 200, 300)", mockContext)).toBe(600);
      expect(evaluateSheetFormula("=AVERAGE(10, 20, 30)", mockContext)).toBe(20);
      expect(evaluateSheetFormula("=MAX(5, 50, 25)", mockContext)).toBe(50);
      expect(evaluateSheetFormula("=IF(10 > 5, 1000, 2000)", mockContext)).toBe(1000);
      expect(evaluateSheetFormula("=IF(5 > 10, 1000, 2000)", mockContext)).toBe(2000);
    });

    it("evaluates contextual column references dynamically", () => {
      // baseSalary (3000000) + fixedAllow (300000)
      const result = evaluateSheetFormula("=[baseSalary] + [fixedAllow]", mockContext);
      expect(result).toBe(3300000);
    });

    it("handles division by zero and malformed syntax safely without uncaught exceptions", () => {
      // Division by zero should safely return 0 or Infinity without throwing
      const divZero = evaluateSheetFormula("=100 / 0", mockContext);
      expect(divZero === 0 || !Number.isFinite(Number(divZero)) || Number.isNaN(Number(divZero))).toBe(true);

      // Incomplete syntax
      const incomplete = evaluateSheetFormula("=SUM(100,", mockContext);
      expect(incomplete).toBeDefined();

      // Unknown function call
      const unknownFn = evaluateSheetFormula("=CORRUPT_FUNCTION_CALL(123)", mockContext);
      expect(unknownFn).toBeDefined();
    });
  });

  describe("3. Multi-Cell TSV Clipboard Paste & Injection Fuzzing", () => {
    it("parses diverse TSV structures including spaces, trailing tabs, and newlines", () => {
      const rawTsv = "김철수\t3,500,000\t200,000\n이영희\t4,200,000\t300,000\n";
      const rows = parseTsvClipboard(rawTsv);
      expect(rows.length).toBe(2);
      expect(rows[0]).toEqual(["김철수", "3,500,000", "200,000"]);
      expect(rows[1]).toEqual(["이영희", "4,200,000", "300,000"]);
    });

    it("sanitizes currency strings with diverse symbols, commas, won symbols, and whitespace", () => {
      expect(parseMoneyInput("3,500,000원")).toBe(3500000);
      expect(parseMoneyInput("₩4,200,000")).toBe(4200000);
      expect(parseMoneyInput("  500000  ")).toBe(500000);
      expect(parseMoneyInput("-100,000")).toBe(-100000);
      expect(parseMoneyInput("INVALID_TEXT")).toBe(null);
    });
  });

  describe("4. Cryptographic Hash Chain Tamper Detection", () => {
    it("builds a valid hash chain and successfully detects middle-of-chain data tampering", () => {
      const chain: AuditEvent[] = [];

      // Build a chain of 10 dynamic audit events
      for (let i = 0; i < 10; i++) {
        const ev = appendDynamicAuditEvent(chain, {
          actorId: `user_${i}`,
          actorName: `작업자_${i}`,
          action: `action:test:${i}`,
          targetKind: "PS",
          targetCode: `PS-2026-0${i}`,
          policyDecision: "PERMIT",
          reason: `정상 정산 트랜잭션 #${i}`,
        });
        chain.push(ev);
      }

      expect(chain.length).toBe(10);
      const initialVerification = verifyAuditChainIntegrity(chain);
      expect(initialVerification.valid).toBe(true);

      // Adversarial mutation: Alter the action payload of event #5
      const tamperedChain = [...chain];
      tamperedChain[5] = {
        ...tamperedChain[5],
        action: "action:malicious_unauthorized_override",
      };

      const tamperedVerification = verifyAuditChainIntegrity(tamperedChain);
      expect(tamperedVerification.valid).toBe(false);
      expect(tamperedVerification.corruptedIndex).toBe(5);
      expect(tamperedVerification.reason).toContain("Cryptographic payload hash mismatch");
    });

    it("detects broken hash chain pointer (prevHash mismatch)", () => {
      const chain: AuditEvent[] = [];

      for (let i = 0; i < 5; i++) {
        chain.push(
          appendDynamicAuditEvent(chain, {
            actorId: `user_${i}`,
            actorName: `작업자_${i}`,
            action: `action:test:${i}`,
            targetKind: "AP",
            targetCode: `AP-00${i}`,
          })
        );
      }

      // Adversarial mutation: Corrupt prevHash at event #3
      const brokenChain = [...chain];
      brokenChain[3] = {
        ...brokenChain[3],
        prevHash: "tampered_prev_hash_1234567890",
      };

      const verification = verifyAuditChainIntegrity(brokenChain);
      expect(verification.valid).toBe(false);
      expect(verification.corruptedIndex).toBe(3);
      expect(verification.reason).toContain("Previous hash mismatch");
    });
  });

  describe("5. Statutory Tax Truncation & Fiscal Integrity (National Treasury Admin Act §47(1))", () => {
    it("enforces 10-won round down truncation across 100 randomly varied salary profiles", () => {
      const dummyEmp: Employee = {
        id: "emp_fuzz_tax",
        code: "OYT-FUZZ",
        name: "세무검증",
        email: "fuzz@oyatie.com",
        phone: "010-1234-5678",
        entity: "(주)오야티 코퍼레이션",
        site: "인천 제1물류센터",
        dept: "재경팀",
        role: "담당",
        position: "담당",
        empType: "정규",
        baseSalary: 3000000,
        fixedAllow: 200000,
        joinedDate: "2024-01-01",
        status: "재직",
        dependents: 1,
      };

      for (let i = 1; i <= 100; i++) {
        // Vary salary and overtime hours
        const testBase = 2000000 + i * 37391; // irregular number
        const testOt = (i % 15) * 1.5;
        const testNight = (i % 7) * 2;

        const payslip = computeEmployeePayslip(
          { ...dummyEmp, baseSalary: testBase },
          { otHours: testOt, nightHours: testNight, holidayHours: 0 }
        );

        // National Treasury Administration Act §47(1):
        // Income Tax, Local Tax, and final Net Pay MUST have a remainder of 0 when modulo 10
        expect(payslip.netPay % 10).toBe(0);

        const incTax = payslip.deductions.find((d) => d.code === "tax" || d.label === "소득세");
        const locTax = payslip.deductions.find((d) => d.code === "local" || d.label === "지방소득세");

        if (incTax) {
          expect(incTax.amt % 10).toBe(0);
        }
        if (locTax) {
          expect(locTax.amt % 10).toBe(0);
        }
      }
    });
  });

  describe("6. Multi-Year Statutory Evolution Engine (2024 ~ 2028 Horizon)", () => {
    it("dynamically resolves historical, current, and future statutory rate schedules", () => {
      const schedule2024 = resolveStatutoryRates("2024-05");
      const schedule2025 = resolveStatutoryRates("2025-08");
      const schedule2026 = resolveStatutoryRates("2026-07");
      const schedule2027 = resolveStatutoryRates("2027-01");
      const schedule2028 = resolveStatutoryRates("2028-12");

      expect(schedule2024.version).toContain("2024");
      expect(schedule2025.version).toContain("2025");
      expect(schedule2026.version).toContain("2026");
      expect(schedule2027.version).toContain("2027");
      expect(schedule2028.version).toContain("2028");

      // Health insurance rates follow upward statutory trend
      expect(schedule2026.hiTotal).toBeGreaterThanOrEqual(schedule2024.hiTotal);
      expect(schedule2028.hiTotal).toBeGreaterThanOrEqual(schedule2026.hiTotal);
    });
  });
});
