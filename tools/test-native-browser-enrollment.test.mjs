// Real runner/process fault checks only. No native API or enrollment acceptance.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { superviseEnrollmentFault } from "./enrollment-fault-deadlines.mjs";

const env = Object.fromEntries(
  ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "PLAYWRIGHT_BROWSERS_PATH"]
    .filter((key) => process.env[key] !== undefined)
    .map((key) => [key, process.env[key]]),
);

for (const fault of ["signal-after-origin", "eof-after-ready", "oversized", "wrong-version", "wrong-sequence", "wrong-kind", "legacy-selection", "missing-selection", "malformed-selection", "weak-uv"]) {
  test(`enrollment runner confirms cleanup after ${fault}`, { timeout: 40000 }, async () => {
    const child = spawn(process.execPath, [fileURLToPath(new URL("./test-native-browser-enrollment.mjs", import.meta.url))], {
      env, stdio: ["pipe", "pipe", "pipe"],
    });
    // Drain without retaining error stacks, server output or proof bytes.
    let stderrBytes = 0;
    child.stderr.on("data", (bytes) => { stderrBytes += bytes.length; });
    const exited = once(child, "exit");
    const input = child.stdout[Symbol.asyncIterator]();
    let buffered = Buffer.alloc(0);
    let sequence = 0;
    async function read() {
      while (true) {
        if (buffered.length >= 4) {
          const length = buffered.readUInt32BE();
          assert.ok(length > 0 && length <= 65536);
          if (buffered.length >= length + 4) {
            const frame = JSON.parse(buffered.subarray(4, length + 4).toString("utf8"));
            buffered = buffered.subarray(length + 4);
            assert.equal(frame.v, 1);
            assert.equal(frame.seq, sequence++);
            assert.ok(sequence <= 128);
            return frame;
          }
        }
        const next = await input.next();
        assert.ok(!next.done, "runner ended before cleanup acknowledgement");
        assert.ok(buffered.length + next.value.length <= 65540);
        buffered = Buffer.concat([buffered, next.value]);
      }
    }
    function send(frame) {
      const body = Buffer.from(JSON.stringify(frame));
      const header = Buffer.alloc(4);
      header.writeUInt32BE(body.length);
      child.stdin.write(Buffer.concat([header, body]));
    }
    // Parent timeout must request the runner's actual cleanup owner. A forced
    // termination cannot count as confirmed cleanup or a passed fault check.
    const supervision = superviseEnrollmentFault(child);
    try {
      assert.equal((await read()).kind, "origin");
      if (fault === "signal-after-origin") {
        supervision.beginFault();
        child.kill("SIGTERM");
      } else {
        assert.equal((await read()).kind, "ready");
        supervision.beginFault();
        if (fault === "eof-after-ready") child.stdin.end();
        else if (fault === "oversized") {
          const header = Buffer.alloc(4);
          header.writeUInt32BE(65537);
          child.stdin.write(header);
        } else if (["legacy-selection", "missing-selection", "malformed-selection", "weak-uv"].includes(fault)) {
          const selection = fault === "legacy-selection"
            ? { residentKey: "discouraged", requireResidentKey: false, userVerification: "required" }
            : { residentKey: "required", requireResidentKey: fault === "malformed-selection" ? "true" : true,
                userVerification: fault === "weak-uv" ? "preferred" : "required" };
          send({ v: 1, seq: 0, kind: "register", actor: "a", mode: "discoverable",
            ceremony: "11111111-1111-4111-8111-111111111111",
            options: { publicKey: { rp: { id: "localhost" },
              ...(fault === "missing-selection" ? {} : { authenticatorSelection: selection }) } } });
        } else send({ v: fault === "wrong-version" ? 2 : 1,
          seq: fault === "wrong-sequence" ? 1 : 0, kind: fault === "wrong-kind" ? "done" : "register", actor: "a", mode: "legacy",
          ceremony: "11111111-1111-4111-8111-111111111111",
          options: { publicKey: { rp: { id: "localhost" } } } });
      }
      let cleaned = await read();
      // A startup ready frame may already have entered the pipe before SIGTERM.
      if (cleaned.kind === "ready" && fault === "signal-after-origin") cleaned = await read();
      assert.equal(cleaned.kind, "cleaned");
      assert.equal(cleaned.creation_attempts, 0, "invalid contract cannot reach credential creation");
      const aborted = fault === "signal-after-origin" || fault === "eof-after-ready";
      assert.equal(cleaned.outcome, aborted ? "aborted" : "failed");
      child.stdin.end();
      const [code, signal] = await exited;
      assert.equal(code, aborted ? 2 : 1);
      assert.equal(signal, null);
      assert.ok(stderrBytes <= 1048576);
      assert.equal(supervision.forced, false, "fallback intervention cannot prove requested cleanup");
    } finally {
      supervision.clear();
      child.stdin.destroy();
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGTERM");
        const forced = setTimeout(() => child.kill("SIGKILL"), 6000);
        await exited;
        clearTimeout(forced);
      }
    }
  });
}
