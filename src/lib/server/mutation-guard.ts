import { timingSafeEqual } from "node:crypto";

/** A transport check only; callers must still resolve the live session and authorize the action. */
export function hasTrustedMutationProvenance(
  headers: Headers,
  publicOrigin: string | undefined,
  expectedCsrfToken: string | undefined,
): boolean {
  if (!publicOrigin || !expectedCsrfToken) return false;
  try {
    const configured = new URL(publicOrigin);
    const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(configured.hostname);
    if (
      configured.origin !== publicOrigin ||
      (configured.protocol !== "https:" && !(loopback && configured.protocol === "http:"))
    ) return false;
  } catch {
    return false;
  }

  if (headers.get("origin") !== publicOrigin) return false;
  const received = headers.get("x-csrf-token");
  if (
    !received ||
    !/^[A-Za-z0-9_-]{32,128}$/.test(expectedCsrfToken) ||
    !/^[A-Za-z0-9_-]{32,128}$/.test(received) ||
    received.length !== expectedCsrfToken.length
  ) return false;
  return timingSafeEqual(Buffer.from(received), Buffer.from(expectedCsrfToken));
}
