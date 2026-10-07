import { z } from "zod";
import { BrowserSessionError, browserPayslips, canonicalUuid, expirySchema } from "./browser-session";

const instant = expirySchema;
const money = z.string().max(20).regex(/^(?:0|-?[1-9][0-9]*)$/).refine((value) => {
  if (value.length > 20 || !/^(?:0|-?[1-9][0-9]*)$/.test(value)) return false;
  const amount = BigInt(value);
  return amount >= -9223372036854775808n && amount <= 9223372036854775807n;
});
const summary = z.object({ id: canonicalUuid, title: z.string().min(1).max(1200), created_at: instant }).strict();
const contextFields = { company_id: canonicalUuid, browser_context: canonicalUuid, expires_at: instant };
const deduction = z.object({ code: z.enum(["NATIONAL_PENSION", "HEALTH_INSURANCE", "LONG_TERM_CARE",
  "EMPLOYMENT_INSURANCE", "INCOME_TAX", "LOCAL_INCOME_TAX"]), label_ko: z.string().trim().min(1),
  amount_won: money, source_url: z.string().trim().min(1) }).strict();
const payload = z.object({ run_id: canonicalUuid, line_id: canonicalUuid,
  period_start: z.string().date(), period_end: z.string().date(), gross_won: money,
  total_deductions_won: money, net_won: money, deductions: z.array(deduction).max(6),
  tax_table_version: z.string().trim().min(1), calculation_version: z.number().int().min(1).max(2147483647),
}).strict().refine((data) => {
  if (![data.gross_won, data.total_deductions_won, data.net_won, ...data.deductions.map((row) => row.amount_won)].every((value) => money.safeParse(value).success)) return false;
  return data.period_start <= data.period_end && BigInt(data.gross_won) >= 0n && BigInt(data.total_deductions_won) >= 0n &&
  data.deductions.every((row) => BigInt(row.amount_won) >= 0n) &&
  new Set(data.deductions.map((row) => row.code)).size === data.deductions.length &&
  data.deductions.reduce((total, row) => total + BigInt(row.amount_won), 0n) === BigInt(data.total_deductions_won) &&
  BigInt(data.gross_won) - BigInt(data.total_deductions_won) === BigInt(data.net_won);
});
const list = z.object({ ...contextFields, items: z.array(summary).max(25), next_cursor: canonicalUuid.nullable() }).strict()
  .refine((data) => new Set(data.items.map((row) => row.id)).size === data.items.length &&
    (data.next_cursor === null || (data.items.length === 25 && data.items.at(-1)?.id === data.next_cursor)));
const detail = z.object({ ...contextFields, document: summary.extend({ payload }) }).strict();
function checkContext<T extends { browser_context: string; expires_at: string }>(data: T, context: string): T {
  if (data.browser_context !== canonicalUuid.parse(context)) throw new Error("Invalid context");
  return data;
}
export function decodePayslipList(value: unknown, context: string) { return checkContext(list.parse(value), context); }
export function decodePayslip(value: unknown, context: string, id: string) {
  const data = checkContext(detail.parse(value), context);
  if (data.document.id !== canonicalUuid.parse(id)) throw new Error("Invalid document");
  return data;
}
export function formatWon(value: string) { return new Intl.NumberFormat("ko-KR").format(BigInt(money.parse(value))); }
export function formatIssuedAt(value: string) {
  return new Intl.DateTimeFormat("ko-KR", { timeZone: "Asia/Seoul", dateStyle: "medium", timeStyle: "medium" }).format(new Date(instant.parse(value)));
}
export async function ownPayslipList(headers: Headers, context: string, before?: string) {
  const { response, csrf } = await browserPayslips(headers, context, before);
  try {
    const data = decodePayslipList(response, context);
    if (Date.parse(data.expires_at) <= Date.now()) throw new Error();
    return { data, csrf };
  } catch { throw new BrowserSessionError(502); }
}
export async function ownPayslip(headers: Headers, context: string, id: string) {
  const { response, csrf } = await browserPayslips(headers, context, undefined, id);
  try {
    const data = decodePayslip(response, context, id);
    if (Date.parse(data.expires_at) <= Date.now()) throw new Error();
    return { data, csrf };
  } catch { throw new BrowserSessionError(502); }
}
