// R7c tests for the real deployment closure. Missing packager/build is an
// infrastructure prerequisite, never the behavioral RED admission probe.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, readdir, lstat, realpath, readlink, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const sourceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
async function inventory(directory, prefix = "", stageRoot = directory) {
  const result = [];
  for (const entry of await readdir(directory)) {
    const relative = prefix ? `${prefix}/${entry}` : entry;
    const absolute = path.join(directory, entry);
    const metadata = await lstat(absolute);
    if (metadata.isSymbolicLink()) {
      // Standard npm bin links belong to the freshly installed closure. Their
      // targets must remain inside that owned dependency tree; source links do not.
      assert.ok(relative.startsWith("node_modules/"), `runtime source symlink: ${relative}`);
      const dependencyRoot = await realpath(path.join(stageRoot, "node_modules"));
      const immediate = path.resolve(await realpath(path.dirname(absolute)), await readlink(absolute));
      assert.ok(immediate.startsWith(`${dependencyRoot}${path.sep}`), `outside dependency link hop: ${relative}`);
      const target = await realpath(absolute);
      assert.ok(target.startsWith(`${dependencyRoot}${path.sep}`), `escaping dependency link: ${relative}`);
      assert.ok((await lstat(target)).isFile(), "dependency links do not import outside directories");
      result.push(relative);
      continue;
    }
    if (metadata.isDirectory()) result.push(...await inventory(absolute, relative, stageRoot));
    else { assert.ok(metadata.isFile()); result.push(relative); }
  }
  return result.sort();
}

test("deployment and browser consumers stage the same locked production closure", { timeout: 240000 }, async () => {
  const { stageProductionRuntime, verifyProductionRuntime } = await import("./production-runtime.mjs");
  const owned = await mkdtemp(path.join(os.tmpdir(), "r7-runtime-proof-"));
  const destination = path.join(owned, "runtime");
  try {
    const staged = await stageProductionRuntime({ sourceRoot, destination });
    assert.equal(staged.version, 1);
    assert.deepEqual(await verifyProductionRuntime(destination), staged);
    const files = await inventory(destination);
    for (const required of ["server.mjs", "next.config.mjs", "package.json", "package-lock.json", ".next/BUILD_ID", ".next/server/app-paths-manifest.json"]) {
      assert.ok(files.includes(required), `missing normal runtime input ${required}`);
      assert.equal(digest(await readFile(path.join(destination, required))), staged.files[required]);
    }
    for (const required of ["server.mjs", "next.config.mjs", "package.json", "package-lock.json"]) {
      assert.equal(digest(await readFile(path.join(sourceRoot, required))), digest(await readFile(path.join(destination, required))), `candidate bytes changed for ${required}`);
    }
    for (const relative of files) {
      assert.ok(relative.startsWith(".next/") || relative.startsWith("node_modules/") || relative.startsWith("public/") ||
        ["server.mjs", "next.config.mjs", "package.json", "package-lock.json", "runtime-manifest.json", "LICENSE", "NOTICE"].includes(relative),
        `unreviewed runtime input ${relative}`);
      assert.ok(!/^(src|backend-fork|docs|tests|e2e|\.git|\.next\/(cache|types|dev|standalone))\//.test(relative), `non-runtime input ${relative}`);
      assert.ok(!/(^|\/)\.env(?:\.|$)/.test(relative), "dotenv is never a runtime artifact");
      assert.ok(!/^node_modules\/(?:@playwright\/test|playwright(?:-core)?|vitest|typescript|eslint)\//.test(relative), `developer dependency ${relative}`);
      if (!relative.startsWith("node_modules/") && relative.endsWith(".js")) {
        const body = await readFile(path.join(destination, relative), "utf8");
        for (const marker of ["박지영 수석", "AP-3121", "시연용 화면"]) assert.ok(!body.includes(marker), "prototype business fixtures are absent from the deployable runtime");
      }
    }
    const docker = await readFile(path.join(sourceRoot, "Dockerfile"), "utf8");
    const browser = await readFile(path.join(sourceRoot, "tools/test-native-browser-enrollment.mjs"), "utf8");
    const product = await readFile(path.join(sourceRoot, "tools/test-browser-business-session.mjs"), "utf8");
    for (const consumer of [docker, browser, product]) assert.ok(consumer.includes("production-runtime.mjs"), "all production consumers share the sole closure owner");
    assert.ok(!docker.includes("standalone/server.js") && !browser.includes("standalone/server.js"));
  } finally { await rm(owned, { recursive: true, force: true }); }
});

test("runtime verification rejects changed config, runtime bytes and extra secret files", { timeout: 240000 }, async () => {
  const { stageProductionRuntime, verifyProductionRuntime } = await import("./production-runtime.mjs");
  const owned = await mkdtemp(path.join(os.tmpdir(), "r7-runtime-tamper-"));
  const destination = path.join(owned, "runtime");
  try {
    await stageProductionRuntime({ sourceRoot, destination });
    for (const relative of ["server.mjs", "next.config.mjs", "package-lock.json", ".next/BUILD_ID"]) {
      const target = path.join(destination, relative);
      const original = await readFile(target);
      await writeFile(target, Buffer.concat([original, Buffer.from("\nchanged-runtime-input\n")]));
      await assert.rejects(() => verifyProductionRuntime(destination), "a stale or changed input cannot reuse a manifest");
      await writeFile(target, original);
      await verifyProductionRuntime(destination);
    }
    const unexpected = path.join(destination, ".env");
    await writeFile(unexpected, "TEST_ONLY_SENTINEL=untracked-secret-file\n");
    await assert.rejects(() => verifyProductionRuntime(destination), "unexpected dotenv input is rejected");
    await rm(unexpected);
    await verifyProductionRuntime(destination);
  } finally { await rm(owned, { recursive: true, force: true }); }
});

// Serial with the packaging checks: exercise each actual consumer's failure
// owner, without replacing staging or claiming real business/provider success.
test("storefront consumers release owned staging resources after custody rejection", { timeout: 25000 }, async () => {
  const custodyPath = path.join(sourceRoot, ".next/build-inputs.json");
  const original = await readFile(custodyPath);
  const stale = JSON.parse(original.toString("utf8"));
  stale.version = 0;
  try {
    await writeFile(custodyPath, JSON.stringify(stale));
    for (const consumer of ["test-storefront-form.mjs", "test-storefront-real-service.mjs"]) {
      const owned = await mkdtemp(path.join(os.tmpdir(), "r7-consumer-cleanup-"));
      const child = spawn(process.execPath, [path.join(sourceRoot, "tools", consumer)], {
        cwd: sourceRoot, stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, TMPDIR: owned, REAL_SERVICE_API_ORIGIN: "http://127.0.0.1:1",
          REAL_SERVICE_LISTING_ID: "00000000-0000-4000-8000-000000000001" },
      });
      let stderr = ""; let stdout = ""; let bytes = 0; let forced = false;
      for (const [stream, append] of [[child.stderr, (text) => { stderr += text; }], [child.stdout, (text) => { stdout += text; }]]) {
        stream.on("data", (chunk) => { bytes += chunk.length; if (bytes <= 16384) append(chunk.toString("utf8")); });
      }
      const deadline = setTimeout(() => { forced = true; child.kill("SIGTERM"); }, 5000);
      const force = setTimeout(() => { forced = true; child.kill("SIGKILL"); }, 7000);
      try {
        const { code, signal } = await new Promise((resolve, reject) => {
          child.once("error", reject); child.once("close", (code, signal) => resolve({ code, signal }));
        });
        assert.equal(forced, false, "forced termination cannot prove consumer cleanup");
        assert.equal(signal, null); assert.ok(Number.isInteger(code) && code !== 0);
        assert.ok(bytes <= 16384, "failure diagnostics remain bounded");
        assert.match(stderr, /Stale production source\/build custody/);
        assert.ok(!stdout.includes("passed") && !stdout.includes("DETAIL_READY"), "custody refusal precedes business/browser success");
        assert.deepEqual(await readdir(owned), [], "consumer must remove its own staging parent on failure");
      } finally {
        clearTimeout(deadline); clearTimeout(force);
        await rm(owned, { recursive: true, force: true });
      }
    }
  } finally {
    await writeFile(custodyPath, original);
    assert.equal(digest(await readFile(custodyPath)), digest(original), "restore exact accepted build custody");
  }
});
