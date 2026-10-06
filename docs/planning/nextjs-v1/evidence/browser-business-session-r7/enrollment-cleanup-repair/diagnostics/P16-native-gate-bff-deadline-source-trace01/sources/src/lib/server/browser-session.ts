import { createCipheriv, createDecipheriv, createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { isIP } from "node:net";
import { z } from "zod";
import { backendRequest, backendUrl, boundedBytes } from "./backend";
import { hasTrustedMutationProvenance } from "./mutation-guard";

export const canonicalUuid = z.string().regex(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
function decodeBase64(value: string, bytes?: number): Buffer {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("Invalid encoding");
  const decoded = Buffer.from(value, "base64url");
  if (decoded.toString("base64url") !== value || (bytes !== undefined && decoded.length !== bytes))
    throw new Error("Invalid encoding");
  return decoded;
}
function uuidBytes(value: string): Buffer { return Buffer.from(canonicalUuid.parse(value).replaceAll("-", ""), "hex"); }
function handleBytes(value: string): Buffer {
  if (!value.startsWith("bs1.")) throw new Error("Invalid handle");
  return decodeBase64(value.slice(4), 32);
}
export function preauthCsrf(nonce: string, ceremonyId: string): string {
  return createHash("sha256").update("console/preauth-csrf/v1\0")
    .update(decodeBase64(nonce, 32)).update(uuidBytes(ceremonyId)).digest("base64url");
}
export function sessionCsrf(handle: string, context: string): string {
  return createHash("sha256").update("console/session-csrf/v1\0")
    .update(handleBytes(handle)).update(uuidBytes(context)).digest("base64url");
}
const preauthSchema = z.object({ v: z.literal(1), ceremony_id: canonicalUuid, nonce: z.string(),
  origin: z.string(), expires: z.number().int().safe() }).strict();
type Preauth = z.infer<typeof preauthSchema>;
export function openPreauthEnvelope(value: string, selected: { key: Buffer; origin: string; ceremonyId: string; now: number }): Preauth {
  if (value.length > 1024 || !value.startsWith("pa1.") || selected.key.length !== 32) throw new Error("Invalid envelope");
  const bytes = decodeBase64(value.slice(4));
  if (bytes.length <= 28 || bytes.length > 796) throw new Error("Invalid envelope");
  const decipher = createDecipheriv("aes-256-gcm", selected.key, bytes.subarray(0, 12));
  decipher.setAAD(Buffer.from("console/preauth-envelope/v1\0"));
  decipher.setAuthTag(bytes.subarray(-16));
  const plaintext = Buffer.concat([decipher.update(bytes.subarray(12, -16)), decipher.final()]);
  if (plaintext.length > 768) throw new Error("Invalid envelope");
  const data = preauthSchema.parse(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(plaintext)));
  decodeBase64(data.nonce, 32);
  if (data.ceremony_id !== canonicalUuid.parse(selected.ceremonyId) || data.origin !== selected.origin ||
      data.expires <= selected.now || data.expires > selected.now + 60) throw new Error("Invalid envelope");
  return data;
}
function sealPreauth(data: Preauth, key: Buffer): string {
  const bytes = Buffer.from(JSON.stringify(data));
  if (key.length !== 32 || bytes.length > 768) throw new Error("Invalid envelope");
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  cipher.setAAD(Buffer.from("console/preauth-envelope/v1\0"));
  const value = `pa1.${Buffer.concat([iv, cipher.update(bytes), cipher.final(), cipher.getAuthTag()]).toString("base64url")}`;
  if (value.length > 1024) throw new Error("Invalid envelope");
  return value;
}
export function browserCookieName(kind: "preauth" | "session", context: string, secure: boolean): string {
  return `${secure ? "__Host-console" : "console-loopback"}-${kind}-${canonicalUuid.parse(context)}`;
}
function rawCookies(raw: string | null, secure: boolean): Map<string, string> {
  if (raw === null || raw === "") return new Map();
  if (Buffer.byteLength(raw) > 8192 || /[^\x20-\x7e]/.test(raw)) throw new Error("Invalid cookies");
  const result = new Map<string, string>();
  const prefix = secure ? "__Host-console-" : "console-loopback-";
  for (const item of raw.split(";")) {
    const pair = item.trim(); const equals = pair.indexOf("=");
    if (equals < 1) throw new Error("Invalid cookies");
    const name = pair.slice(0, equals); const value = pair.slice(equals + 1);
    if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name)) throw new Error("Invalid cookies");
    if (!name.startsWith(prefix)) continue;
    const match = name.slice(prefix.length).match(/^(session|preauth)-(.+)$/);
    if (!match || result.has(name)) throw new Error("Invalid cookies");
    canonicalUuid.parse(match[2]);
    if (match[1] === "session") handleBytes(value);
    else {
      if (!value.startsWith("pa1.") || value.length > 1024) throw new Error("Invalid cookies");
      const bytes = decodeBase64(value.slice(4));
      if (bytes.length <= 28 || bytes.length > 796) throw new Error("Invalid cookies");
    }
    result.set(name, value);
  }
  return result;
}
export function selectBrowserCookie(raw: string | null, selected: { kind: "session" | "preauth"; context: string; secure: boolean }): string | null {
  const name = browserCookieName(selected.kind, selected.context, selected.secure);
  return rawCookies(raw, selected.secure).get(name) ?? null;
}
export function browserCookieAdmission(raw: string | null, action: "start" | "finish", secure: boolean): boolean {
  const names = [...rawCookies(raw, secure).keys()];
  const sessionCount = names.filter((name) => name.includes("-session-")).length;
  const preauthCount = names.length - sessionCount;
  return sessionCount < 8 && (action === "finish" || preauthCount < 8);
}

const safeInteger = z.number().int().safe();
const timestampSchema = z.tuple([safeInteger, safeInteger, safeInteger, safeInteger, safeInteger,
  safeInteger, safeInteger, safeInteger, safeInteger]).refine((value) => {
  const [year, ordinal, hour, minute, second, nano, oh, om, os] = value;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const signs = [oh, om, os].filter((x) => x !== 0).map(Math.sign);
  return year >= 1 && year <= 9999 && ordinal >= 1 && ordinal <= (leap ? 366 : 365) &&
    hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59 && second >= 0 && second <= 59 &&
    nano >= 0 && nano < 1e9 && Math.abs(oh) <= 23 && Math.abs(om) <= 59 && Math.abs(os) <= 59 &&
    signs.every((sign) => sign === signs[0]);
});
function rfc3339(value: string): boolean {
  return /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(value) && Number.isFinite(Date.parse(value));
}
const expirySchema = z.string().datetime({ offset: true }).refine(rfc3339);
const historyItemSchema = z.object({
  id: canonicalUuid, employee_id: canonicalUuid, employee_display_name: z.string(),
  kind: z.enum(["CLOCK_IN", "OUT_FOR_WORK", "BUSINESS_TRIP", "RETURNED", "CLOCK_OUT"]),
  state_after: z.enum(["CLOCKED_IN", "OUT_FOR_WORK", "BUSINESS_TRIP", "OFF_DUTY"]),
  occurred_at: timestampSchema, work_date: z.string().date(),
  note: z.string().optional(), payroll_material_ref_id: canonicalUuid.nullable(),
  payroll_link_status: z.enum(["LINKED", "UNLINKED"]), duplicate: z.boolean(),
}).strict().refine((row) => (row.payroll_material_ref_id !== null) === (row.payroll_link_status === "LINKED"));
const ownAttendanceSchema = z.object({
  context: z.object({ company_id: canonicalUuid, company_name: z.string(), account_display_name: z.string(), employee_linked: z.boolean() }).strict(),
  browser_context: canonicalUuid, expires_at: expirySchema,
  history: z.object({ total: safeInteger.nonnegative(), limit: safeInteger.min(1).max(1000),
    offset: safeInteger.nonnegative(), items: z.array(historyItemSchema) }).strict(),
}).strict().refine(({ context, history }) => history.items.length === Math.min(history.limit, Math.max(0, history.total - history.offset)) &&
  (context.employee_linked || (history.total === 0 && history.items.length === 0)));
export function decodeOwnAttendance(value: unknown, context: string) {
  const data = ownAttendanceSchema.parse(value);
  if (data.browser_context !== canonicalUuid.parse(context)) throw new Error("Invalid context");
  return data;
}
export function formatAttendanceInstant(value: unknown): string {
  const [year, ordinal, hour, minute, second, nano, oh, om, os] = timestampSchema.parse(value);
  const date = new Date(0);
  date.setUTCFullYear(year, 0, ordinal); date.setUTCHours(hour, minute, second, Math.floor(nano / 1e6));
  const kst = new Date(date.getTime() - (oh * 3600 + om * 60 + os) * 1000 + 9 * 3600 * 1000);
  return `${kst.toISOString().split(".")[0].replace("T", " ")} (KST)`;
}

export class BrowserSessionError extends Error {
  constructor(public readonly status: number) { super("Browser access unavailable"); }
}
const loopback = (host: string) => host === "localhost" || host === "127.0.0.1" || host === "[::1]";
function secretFromEnv(name: string): Buffer { return decodeBase64(process.env[name] ?? "", 32); }
export function browserIngress(headers: Headers) {
  try {
    const origin = process.env.CONSOLE_PUBLIC_ORIGIN ?? "";
    const url = new URL(origin);
    const secure = url.protocol === "https:";
    if (url.origin !== origin || url.username || url.password ||
        (!secure && !(url.protocol === "http:" && loopback(url.hostname) && process.env.CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP === "true"))) throw new Error();
    const expected = secretFromEnv("CONSOLE_NODE_INGRESS_MARKER");
    const marker = decodeBase64(headers.get("x-console-node-ingress") ?? "", 32);
    const peer = headers.get("x-console-node-peer") ?? "";
    if (!timingSafeEqual(expected, marker) || !isIP(peer) || peer.startsWith("::ffff:") ||
        headers.get("x-console-node-secure") !== (secure ? "1" : "0") || headers.get("host") !== url.host ||
        (!secure && peer !== "127.0.0.1" && peer !== "::1")) throw new Error();
    const backend = backendUrl("/");
    if (backend.protocol !== "https:" && !(backend.protocol === "http:" && loopback(backend.hostname) &&
        process.env.CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP === "true")) throw new Error();
    const service = secretFromEnv("CONSOLE_BROWSER_INGRESS_KEY");
    const key = secretFromEnv("CONSOLE_BROWSER_PREAUTH_KEY");
    return { origin, secure, peer, service, key };
  } catch { throw new BrowserSessionError(503); }
}
const paths = { start: "/api/v1/auth/browser-session/start", login: "/api/v1/auth/browser-session/login",
  logout: "/api/v1/auth/browser-session/logout", history: "/api/v1/hr/browser-session/attendance-records/me" } as const;
async function nativeRequest(action: keyof typeof paths, headers: Headers, body?: unknown): Promise<unknown> {
  const ingress = browserIngress(headers);
  const forwarded = new Headers({ "x-forwarded-for": ingress.peer,
    "x-console-browser-ingress": `bi1.${ingress.service.toString("base64url")}`, Accept: "application/json" });
  for (const name of ["x-device-id", "user-agent", "traceparent"]) {
    const value = headers.get(name); if (value !== null) forwarded.set(name, value);
  }
  if (body !== undefined) forwarded.set("Content-Type", "application/json");
  let response: Response;
  try { response = await backendRequest(paths[action], { method: "POST", headers: forwarded,
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) }); }
  catch { throw new BrowserSessionError(503); }
  if (action === "logout") {
    if (response.status !== 204) { await response.body?.cancel(); throw new BrowserSessionError(response.ok ? 502 : response.status); }
    return undefined;
  }
  if (!response.ok) { await response.body?.cancel(); throw new BrowserSessionError(response.status); }
  try {
    if (response.status !== 200 || response.headers.get("content-type")?.split(";", 1)[0].trim().toLowerCase() !== "application/json") throw new Error();
    const bytes = await boundedBytes(response.body, action === "history" ? 2 * 1024 * 1024 : 65536);
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch { throw new BrowserSessionError(502); }
}
export async function ownAttendance(headers: Headers, context: string, offset: number) {
  const ingress = browserIngress(headers);
  const handle = selectBrowserCookie(headers.get("cookie"), { kind: "session", context, secure: ingress.secure });
  if (!handle) throw new BrowserSessionError(401);
  const response = await nativeRequest("history", headers, { session_token: handle, browser_context: context, limit: 25, offset });
  let data;
  try { data = decodeOwnAttendance(response, context); }
  catch { throw new BrowserSessionError(502); }
  if (data.history.limit !== 25 || data.history.offset !== offset || Date.parse(data.expires_at) <= Date.now()) throw new BrowserSessionError(502);
  return { data, csrf: sessionCsrf(handle, context) };
}
function cookieHeader(kind: "preauth" | "session", context: string, value: string, secure: boolean, expiry?: number): string {
  return `${browserCookieName(kind, context, secure)}=${value}; Path=/; HttpOnly; SameSite=Strict${secure ? "; Secure" : ""}; ${
    expiry === undefined ? "Max-Age=0" : `Expires=${new Date(expiry * 1000).toUTCString()}`}`;
}
export const browserResponseHeaders = { "Cache-Control": "private, no-store", "X-Content-Type-Options": "nosniff" };
function sourceResponse<T>(schema: z.ZodType<T>, value: unknown): T {
  const result = schema.safeParse(value);
  if (!result.success) throw new BrowserSessionError(502);
  return result.data;
}
const admissionMessage = "기존 인증을 완료하거나 해당 화면에서 로그아웃한 뒤 다시 시도하세요. 원래 만료 기한까지 기다릴 수도 있습니다.";
export async function browserAction(action: "start" | "login" | "logout", request: Request): Promise<Response> {
  try {
    const ingress = browserIngress(request.headers);
    if (request.headers.get("origin") !== ingress.origin) throw new BrowserSessionError(403);
    const raw = request.headers.get("cookie");
    const bytes = await boundedBytes(request.body, action === "login" ? 2 * 1024 * 1024 : (action === "start" ? 0 : 4096));
    if (action === "start") {
      if (!browserCookieAdmission(raw, "start", ingress.secure)) return Response.json({ message: admissionMessage }, { status: 429, headers: browserResponseHeaders });
      const start = sourceResponse(z.object({ ceremony_id: canonicalUuid, expires_at: expirySchema,
        challenge: z.object({ publicKey: z.object({ challenge: z.string().min(1), rpId: z.string().min(1),
          allowCredentials: z.array(z.unknown()).length(0), userVerification: z.literal("required") }).passthrough() }).strict() }).strict()
        , await nativeRequest("start", request.headers));
      const now = Math.floor(Date.now() / 1000);
      const expires = Math.min(Math.floor(Date.parse(start.expires_at) / 1000), now + 60);
      if (expires <= now) throw new BrowserSessionError(502);
      const preauth: Preauth = { v: 1, ceremony_id: start.ceremony_id, nonce: randomBytes(32).toString("base64url"), origin: ingress.origin, expires };
      const response = Response.json({ ...start, csrf_token: preauthCsrf(preauth.nonce, preauth.ceremony_id) }, { headers: browserResponseHeaders });
      response.headers.append("Set-Cookie", cookieHeader("preauth", preauth.ceremony_id, sealPreauth(preauth, ingress.key), ingress.secure, expires));
      return response;
    }
    if (!request.headers.get("content-type")?.split(";", 1)[0].includes("application/json")) throw new BrowserSessionError(400);
    const body: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    if (action === "login") {
      if (!browserCookieAdmission(raw, "finish", ingress.secure)) return Response.json({ message: admissionMessage }, { status: 429, headers: browserResponseHeaders });
      // The bounded signed credential goes unchanged to the native WebAuthn
      // owner, which owns its optional extension and authenticator wire fields.
      const finish = z.object({ ceremony_id: canonicalUuid, credential: z.record(z.unknown()) }).strict().parse(body);
      const envelope = selectBrowserCookie(raw, { kind: "preauth", context: finish.ceremony_id, secure: ingress.secure });
      if (!envelope) throw new BrowserSessionError(401);
      const preauth = openPreauthEnvelope(envelope, { key: ingress.key, origin: ingress.origin, ceremonyId: finish.ceremony_id, now: Math.floor(Date.now() / 1000) });
      if (!hasTrustedMutationProvenance(request.headers, ingress.origin, preauthCsrf(preauth.nonce, preauth.ceremony_id))) throw new BrowserSessionError(403);
      const login = sourceResponse(z.object({ session_token: z.string().refine((value) => { try { handleBytes(value); return true; } catch { return false; } }),
        context_id: canonicalUuid, expires_at: expirySchema }).strict(), await nativeRequest("login", request.headers, finish));
      if (login.context_id !== finish.ceremony_id || Date.parse(login.expires_at) <= Date.now()) throw new BrowserSessionError(502);
      const response = Response.json({ context_id: login.context_id, expires_at: login.expires_at }, { headers: browserResponseHeaders });
      response.headers.append("Set-Cookie", cookieHeader("session", login.context_id, login.session_token, ingress.secure, Math.floor(Date.parse(login.expires_at) / 1000)));
      response.headers.append("Set-Cookie", cookieHeader("preauth", preauth.ceremony_id, "", ingress.secure));
      return response;
    }
    const { browser_context: context } = z.object({ browser_context: canonicalUuid }).strict().parse(body);
    const handle = selectBrowserCookie(raw, { kind: "session", context, secure: ingress.secure });
    if (!handle) throw new BrowserSessionError(401);
    if (!hasTrustedMutationProvenance(request.headers, ingress.origin, sessionCsrf(handle, context))) throw new BrowserSessionError(403);
    await nativeRequest("logout", request.headers, { session_token: handle, browser_context: context });
    const response = new Response(null, { status: 204, headers: browserResponseHeaders });
    response.headers.append("Set-Cookie", cookieHeader("session", context, "", ingress.secure));
    return response;
  } catch (error) {
    const status = error instanceof BrowserSessionError ? error.status : error instanceof RangeError ? 413 : 400;
    return Response.json({ message: status >= 500 ? "결과를 확인할 수 없습니다. 로그인은 새 인증으로, 로그아웃은 같은 화면에서 다시 확인하세요." :
      "요청 또는 접근 권한을 확인할 수 없습니다. 인증 기한과 입력을 확인하고 다시 로그인하세요." }, { status, headers: browserResponseHeaders });
  }
}
