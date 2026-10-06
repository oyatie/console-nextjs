// R7c test preparation. Missing production helpers are a build prerequisite,
// never the executable missing-route RED used for implementation admission.
import { createCipheriv } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
  preauthCsrf, sessionCsrf, openPreauthEnvelope, selectBrowserCookie,
  browserCookieAdmission, decodeOwnAttendance, formatAttendanceInstant,
} from "@/lib/server/browser-session";

const context = "40414243-4445-4647-4849-4a4b4c4d4e4f";
const other = "00010203-0405-0607-0809-0a0b0c0d0e0f";
const origin = "https://console.example.test";
const nonce = Buffer.from(Array.from({ length: 32 }, (_, i) => i)).toString("base64url");
const handle = `bs1.${nonce}`;
const key = Buffer.from(Array.from({ length: 32 }, (_, i) => i));
const now = 1_800_000_000;
const payload = { v: 1, ceremony_id: context, nonce, origin, expires: now + 59 };

// Independent test encoding: production open must authenticate these actual
// AES-GCM bytes, not call a test shim or compare its own encoder with itself.
function envelope(value: unknown, secret = key, aad = "console/preauth-envelope/v1\0") {
  const iv = Buffer.from(Array.from({ length: 12 }, (_, i) => i));
  const cipher = createCipheriv("aes-256-gcm", secret, iv);
  cipher.setAAD(Buffer.from(aad));
  const ciphertext = Buffer.concat([cipher.update(JSON.stringify(value), "utf8"), cipher.final()]);
  return `pa1.${Buffer.concat([iv, ciphertext, cipher.getAuthTag()]).toString("base64url")}`;
}

function sessionCookie(id = context, value = handle) { return `__Host-console-session-${id}=${value}`; }
function preauthCookie(id = context) { return `__Host-console-preauth-${id}=${envelope({ ...payload, ceremony_id: id })}`; }
function contexts(count: number) {
  return Array.from({ length: count }, (_, i) => `00000000-0000-4000-8000-${String(i + 1).padStart(12, "0")}`);
}

function nativeHistory(occurredAt: unknown = [2024, 60, 23, 59, 59, 999999999, 0, 0, 0]) {
  return {
    context: { company_id: other, company_name: "검증 법인", account_display_name: "검증 주체", employee_linked: true },
    browser_context: context, expires_at: "2027-01-15T08:05:59Z",
    history: { total: 1, limit: 25, offset: 0, items: [{
      id: "00000000-0000-4000-8000-000000000001", employee_id: other,
      employee_display_name: "검증 주체", kind: "CLOCK_IN", state_after: "CLOCKED_IN",
      occurred_at: occurredAt, work_date: "2024-03-01",
      payroll_material_ref_id: null, payroll_link_status: "UNLINKED", duplicate: false,
    }] },
  };
}

describe("native browser boundary", () => {
  it("pins independent CSRF literals and binds the exact secret and UUID", () => {
    expect(preauthCsrf(nonce, context)).toBe("2EJ46dnli4ZrEz5ejhvGR1kEYXozOoNr1jwwnU8J1C4");
    expect(sessionCsrf(handle, context)).toBe("rWMekY7qYbHU0GK9KQAnOsXq4iK4h6yuGDYQO5oC2RU");
    expect(preauthCsrf(nonce, other)).not.toBe(preauthCsrf(nonce, context));
    expect(sessionCsrf(handle, other)).not.toBe(sessionCsrf(handle, context));
    expect(preauthCsrf(nonce, context)).not.toBe(sessionCsrf(handle, context));
    const changedBytes = Buffer.from(nonce, "base64url");
    changedBytes[0] ^= 1;
    const changedNonce = changedBytes.toString("base64url");
    expect(preauthCsrf(changedNonce, context)).not.toBe(preauthCsrf(nonce, context));
    expect(sessionCsrf(`bs1.${changedNonce}`, context)).not.toBe(sessionCsrf(handle, context));
    for (const candidate of [`${nonce}=`, nonce.slice(1), "!".repeat(43)]) {
      expect(() => preauthCsrf(candidate, context)).toThrow();
      expect(() => sessionCsrf(`bs1.${candidate}`, context)).toThrow();
    }
    for (const candidate of [context.toUpperCase(), `${context} `, "invalid"]) {
      expect(() => sessionCsrf(handle, candidate)).toThrow();
    }
  });

  it("authenticates independent preauth bytes and rejects every binding and expiry fault", () => {
    const selected = { key, origin, ceremonyId: context, now };
    expect(openPreauthEnvelope(envelope(payload), selected)).toEqual(payload);
    for (const value of [
      { ...payload, v: 2 }, { ...payload, ceremony_id: other },
      { ...payload, nonce: `${nonce}=` }, { ...payload, origin: `${origin}/` },
      { ...payload, expires: now }, { ...payload, expires: now - 1 },
      { ...payload, expires: now + 0.5 }, { ...payload, expires: String(now + 59) },
      { ...payload, extra: "hidden" }, { ...payload, nonce: "x".repeat(769) },
    ]) expect(() => openPreauthEnvelope(envelope(value), selected)).toThrow();
    for (const token of [
      `${envelope(payload)}=`, envelope(payload).replace("pa1.", "pa2."),
      `pa1.${"x".repeat(1021)}`, "pa1.", envelope(payload, Buffer.alloc(32)),
      envelope(payload, key, "console/preauth-envelope/v2\0"),
    ]) expect(() => openPreauthEnvelope(token, selected)).toThrow();
    const altered = Buffer.from(envelope(payload).slice(4), "base64url");
    for (const index of [0, 12, altered.length - 1]) {
      const bytes = Buffer.from(altered); bytes[index] ^= 1;
      expect(() => openPreauthEnvelope(`pa1.${bytes.toString("base64url")}`, selected)).toThrow();
    }
    expect(() => openPreauthEnvelope(envelope(payload), { ...selected, ceremonyId: other })).toThrow();
    expect(() => openPreauthEnvelope(envelope(payload), { ...selected, now: payload.expires })).toThrow();
  });

  it("selects only the exact raw cookie generation and denies duplicates before parsed lookup", () => {
    const selected = { kind: "session" as const, context, secure: true };
    expect(selectBrowserCookie(`${sessionCookie(other)}; ${sessionCookie()}`, selected)).toBe(handle);
    expect(selectBrowserCookie(sessionCookie(other), selected)).toBeNull();
    expect(selectBrowserCookie(null, selected)).toBeNull();
    for (const raw of [
      `${sessionCookie()}; ${sessionCookie()}`,
      `${sessionCookie()}; ${sessionCookie(context, "bs1.invalid")}`,
      sessionCookie(context, `${handle}=`), sessionCookie(context, "v1.invalid"),
      sessionCookie(context.toUpperCase()), `${sessionCookie()}; __Host-console-session-invalid=${handle}`,
      `${sessionCookie()}; unbounded=${"x".repeat(8192)}`,
    ]) expect(() => selectBrowserCookie(raw, selected)).toThrow();
    const prefix = `${sessionCookie()}; unrelated=`;
    expect(selectBrowserCookie(prefix + "x".repeat(8192 - Buffer.byteLength(prefix)), selected)).toBe(handle);
    expect(() => selectBrowserCookie(prefix + "x".repeat(8193 - Buffer.byteLength(prefix)), selected)).toThrow();
    expect(selectBrowserCookie(preauthCookie(), { kind: "preauth", context, secure: true })).toBe(envelope(payload));
  });

  it("distinguishes bounded cookie growth from finishing an already admitted ceremony", () => {
    const ids = contexts(8);
    const preauth = ids.map(preauthCookie).join("; ");
    const sessions = ids.map((id) => sessionCookie(id)).join("; ");
    expect(browserCookieAdmission(preauth, "start", true)).toBe(false);
    expect(browserCookieAdmission(preauth, "finish", true)).toBe(true);
    expect(browserCookieAdmission(sessions, "start", true)).toBe(false);
    expect(browserCookieAdmission(sessions, "finish", true)).toBe(false);
    expect(browserCookieAdmission(ids.slice(0, 7).map(preauthCookie).join("; "), "start", true)).toBe(true);
    expect(browserCookieAdmission(`${preauth}; ${ids.slice(0, 7).map((id) => sessionCookie(id)).join("; ")}`, "finish", true)).toBe(true);
  });

  it("rejects native calendar dates that JavaScript would silently normalize", () => {
    const data = nativeHistory();
    for (const expires_at of ["2027-02-31T08:05:59Z", "2027-02-29T08:05:59Z", "2027-01-15T24:00:00Z"]) {
      expect(() => decodeOwnAttendance({ ...data, expires_at }, context)).toThrow();
    }
    for (const work_date of ["2027-02-31", "2027-02-29"]) {
      expect(() => decodeOwnAttendance({ ...data, history: { ...data.history,
        items: [{ ...data.history.items[0], work_date }] } }, context)).toThrow();
    }
    expect(decodeOwnAttendance({ ...data, expires_at: "2028-02-29T23:59:59.123456789+09:00" }, context).expires_at)
      .toBe("2028-02-29T23:59:59.123456789+09:00");
  });

  it("validates the actual nine-item timestamps and exact authorized own projection", () => {
    const data = nativeHistory();
    expect(decodeOwnAttendance(data, context)).toEqual(data);
    expect(formatAttendanceInstant(data.history.items[0].occurred_at)).toBe("2024-03-01 08:59:59 (KST)");
    const lastYear = decodeOwnAttendance(nativeHistory([9999, 365, 23, 59, 59, 0, 0, 0, 0]), context);
    expect(formatAttendanceInstant(lastYear.history.items[0].occurred_at)).toBe("+010000-01-01 08:59:59 (KST)");
    for (const timestamp of [
      [2023, 366, 0, 0, 0, 0, 0, 0, 0], [2024, 0, 0, 0, 0, 0, 0, 0, 0],
      [2024, 60, 24, 0, 0, 0, 0, 0, 0], [2024, 60, 0, 60, 0, 0, 0, 0, 0],
      [2024, 60, 0, 0, 60, 0, 0, 0, 0], [2024, 60, 0, 0, 0, 1000000000, 0, 0, 0],
      [2024, 60, 0, 0, 0, -1, 0, 0, 0], [2024, 60, 0, 0, 0, 0, 24, 0, 0],
      [2024, 60, 0, 0, 0, 0, 9, -1, 0], [2024, 60, 0, 0, 0, 0, 0, 0],
      "2024-02-29T00:00:00Z", [2024, 60, 0.5, 0, 0, 0, 0, 0, 0],
    ]) expect(() => decodeOwnAttendance(nativeHistory(timestamp), context)).toThrow();
    for (const value of [
      { ...data, browser_context: other }, { ...data, access_token: "forbidden" },
      { ...data, context: { ...data.context, employee_linked: "true" } },
      { ...data, history: { ...data.history, total: Number.MAX_SAFE_INTEGER + 1 } },
      { ...data, history: { ...data.history, offset: -1 } },
      { ...data, history: { ...data.history, limit: 1001 } },
      { ...data, context: { ...data.context, employee_linked: false } },
    ]) expect(() => decodeOwnAttendance(value, context)).toThrow();
  });
});
