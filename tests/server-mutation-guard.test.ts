import { describe, expect, it } from "vitest";
import { hasTrustedMutationProvenance } from "@/lib/server/mutation-guard";

const origin = "https://console.example.test";
const csrf = "9b0fc126a807c7f6de33c0dc76615d1b";

function headers(from: string | null, token: string | null, host = "console.example.test") {
  const result = new Headers({ host });
  if (from !== null) result.set("origin", from);
  if (token !== null) result.set("x-csrf-token", token);
  return result;
}

describe("session mutation provenance", () => {
  it("accepts the configured origin with the live session token", () => {
    expect(hasTrustedMutationProvenance(headers(origin, csrf), origin, csrf)).toBe(true);
  });

  it("rejects missing, sibling, null and malformed origins regardless of Host", () => {
    for (const from of [null, "https://other.example.test", "null", `${origin}/path`]) {
      expect(hasTrustedMutationProvenance(headers(from, csrf), origin, csrf)).toBe(false);
      expect(hasTrustedMutationProvenance(headers(from, csrf, "other.example.test"), origin, csrf)).toBe(false);
    }
  });

  it("rejects missing or mismatched CSRF tokens", () => {
    for (const token of [null, "", `${csrf}a`, "ñ".repeat(csrf.length)]) {
      expect(hasTrustedMutationProvenance(headers(origin, token), origin, csrf)).toBe(false);
    }
  });

  it("fails closed on an absent or malformed deployment origin", () => {
    for (const configured of ["", "https://console.example.test/path", "http://console.example.test"]) {
      expect(hasTrustedMutationProvenance(headers(origin, csrf), configured, csrf)).toBe(false);
    }
  });
});
