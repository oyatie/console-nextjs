// R7c tests for the real deployment closure. Missing packager/build is an
// infrastructure prerequisite, never the behavioral RED admission probe.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
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
