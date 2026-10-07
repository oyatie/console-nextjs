import { types } from "node:util";

const sources = [
  "test-browser-business-session.mjs",
  "browser-business-boundary.mjs",
  "browser-business-temporal.mjs",
  "browser-business-restore.mjs",
  "browser-payslips.mjs",
].map((file) => ({ file, url: new URL(file, import.meta.url).href }));

// Never report the error message, raw stack, paths, or assertion values. Only
// exact checked-in probe frames may produce a bounded source location.
export function browserFailureLocation(error) {
  try {
    if (!types.isNativeError(error)) return undefined;
    const message = Object.getOwnPropertyDescriptor(error, "message")?.value;
    if (typeof message !== "string") return undefined;
    if (!Object.getOwnPropertyDescriptor(error, "stack")) return undefined;
    const stack = error.stack;
    if (typeof stack !== "string" || stack.length > 65536) return undefined;
    // Assertion messages can contain newlines and frame-shaped private values.
    // Consume the entire verified header before looking at any stack frames.
    const firstNewline = stack.indexOf("\n");
    if (firstNewline < 0) return undefined;
    const start = stack.lastIndexOf(message, firstNewline);
    if (start < 0 || start > 512 || stack.slice(0, start).includes("\n")) return undefined;
    const frames = stack.slice(start + message.length);
    if (!frames.startsWith("\n")) return undefined;
    for (const frame of frames.slice(1).split("\n")) {
      if (!/^\s+at /.test(frame)) continue;
      for (const { file, url } of sources) {
        const prefix = frame.startsWith(`    at ${url}:`) ? `    at ${url}:` : `(${url}:`;
        const offset = prefix.startsWith("(") ? frame.lastIndexOf(prefix) : 0;
        if (offset < 0) continue;
        const location = frame.slice(offset + prefix.length);
        const match = /^([1-9]\d{0,3}):([1-9]\d{0,3})\)?$/.exec(location);
        if (match) return { file, line: Number(match[1]), column: Number(match[2]) };
      }
    }
  } catch { /* Diagnostics cannot replace the original failure or cleanup. */ }
  return undefined;
}
