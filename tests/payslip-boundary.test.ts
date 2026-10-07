import { describe, expect, it } from "vitest";
import { decodePayslip, decodePayslipList, formatWon } from "@/lib/server/payslips";
const context = "00000000-0000-4000-8000-000000000001";
const id = "00000000-0000-4000-8000-000000000002";
const fields = { company_id: id, browser_context: context, expires_at: "2027-01-01T00:00:00Z" };
const summary = { id, title: "발급된 명세서", created_at: "2026-09-30T00:00:00Z" };
const payload = { run_id: id, line_id: context, period_start: "2026-09-01", period_end: "2026-09-30",
  gross_won: "9007199254740993", total_deductions_won: "1", net_won: "9007199254740992",
  deductions: [{ code: "INCOME_TAX", label_ko: "소득세", amount_won: "1", source_url: "https://www.nts.go.kr/" }],
  calculation_version: 1, tax_table_version: "test-only" };
const document = { ...summary, payload };
describe("issued payslip transport", () => {
  it("preserves integer precision and exact own document/context binding", () => {
    expect(decodePayslip({ ...fields, document }, context, id).document.payload).toEqual(payload);
    expect(formatWon(payload.gross_won)).toBe("9,007,199,254,740,993");
    expect(formatWon("-9223372036854775808")).toBe("-9,223,372,036,854,775,808");
    expect(() => decodePayslip({ ...fields, document }, id, id)).toThrow();
    expect(() => decodePayslip({ ...fields, document }, context, context)).toThrow();
  });
  it("rejects malformed or inconsistent financial data rather than rounding or filling zeros", () => {
    for (const [field, value] of [["gross_won", 9007199254740992], ["gross_won", "9223372036854775808"],
      ["gross_won", "1.0"], ["gross_won", "01"], ["gross_won", "-0"], ["net_won", null],
      ["net_won", "0"], ["period_end", "2026-02-31"], ["period_end", "2025-01-01"],
      ["calculation_version", 0], ["tax_table_version", ""], ["extra", 1]]) {
      expect(() => decodePayslip({ ...fields, document: { ...summary, payload: { ...payload, [String(field)]: value } } }, context, id)).toThrow();
    }
    expect(() => decodePayslip({ ...fields, document: { ...summary, payload: { ...payload,
      deductions: [payload.deductions[0], payload.deductions[0]] } } }, context, id)).toThrow();
  });
  it("rejects balanced negative producer amounts and invalid timestamps, while preserving negative net", () => {
    for (const invalid of [
      { ...payload, gross_won: "-1", total_deductions_won: "0", net_won: "-1", deductions: [] },
      { ...payload, gross_won: "1", total_deductions_won: "-1", net_won: "2", deductions: [{ ...payload.deductions[0], amount_won: "-1" }] },
    ]) expect(() => decodePayslip({ ...fields, document: { ...summary, payload: invalid } }, context, id)).toThrow();
    expect(decodePayslip({ ...fields, document: { ...summary, payload: { ...payload,
      gross_won: "0", total_deductions_won: "1", net_won: "-1" } } }, context, id).document.payload.net_won).toBe("-1");
    for (const suffix of ["+99:99", "+25:00", "+00:60"]) {
      const invalid = `2026-09-30T00:00:00${suffix}`;
      expect(() => decodePayslip({ ...fields, expires_at: invalid, document }, context, id)).toThrow();
      expect(() => decodePayslip({ ...fields, document: { ...document, created_at: invalid } }, context, id)).toThrow();
      expect(() => decodePayslipList({ ...fields, items: [{ ...summary, created_at: invalid }], next_cursor: null }, context)).toThrow();
    }
  });
  it("requires terminal cursors and unique bounded list items without payload leakage", () => {
    expect(decodePayslipList({ ...fields, items: [summary], next_cursor: null }, context).items).toEqual([summary]);
    for (const data of [
      { ...fields, items: [summary], next_cursor: id }, { ...fields, items: [summary, summary], next_cursor: null },
      { ...fields, items: [document], next_cursor: null }, { ...fields, items: [], next_cursor: null, access_token: "forbidden" },
      { ...fields, items: [], next_cursor: null, browser_context: id },
    ]) expect(() => decodePayslipList(data, context)).toThrow();
  });
});
