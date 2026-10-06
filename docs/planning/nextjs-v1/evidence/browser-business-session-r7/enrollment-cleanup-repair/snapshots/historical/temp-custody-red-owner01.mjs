import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { copyFile, lstat, mkdir, readFile, readdir, readlink, realpath, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runtimeInventory as inventory, topFiles, verifyRuntimeClosure } from "../server.mjs";
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const buildRootFiles = new Set(["BUILD_ID", "app-path-routes-manifest.json", "build-manifest.json", "prerender-manifest.json", "react-loadable-manifest.json", "required-server-files.json", "routes-manifest.json", "package.json"]);
async function buildOutputFiles(root) {
  const files = {};
  for (const entry of (await readdir(path.join(root, ".next"))).sort()) {
    if (buildRootFiles.has(entry)) {
      const relative = `.next/${entry}`;
      if (!(await lstat(path.join(root, relative))).isFile()) throw new Error("Invalid build output");
      files[relative] = sha(await readFile(path.join(root, relative)));
    } else if (entry === "server" || entry === "static") Object.assign(files, await inventory(root, `.next/${entry}`));
  }
  return files;
}
async function copyTree(source, destination, signal) {
  signal?.throwIfAborted();
  const metadata = await lstat(source);
  if (metadata.isSymbolicLink()) throw new Error("Source symlink refused");
  if (metadata.isDirectory()) {
    await mkdir(destination, { recursive: true });
    for (const entry of (await readdir(source)).sort()) await copyTree(path.join(source, entry), path.join(destination, entry), signal);
  } else {
    if (!metadata.isFile()) throw new Error("Non-file source input");
    await copyFile(source, destination);
  }
}
async function optionalCopy(source, destination, signal) {
  try { await lstat(source); } catch (error) { if (error.code === "ENOENT") return; throw error; }
  await copyTree(source, destination, signal);
}
async function install(destination, signal) {
  signal?.throwIfAborted();
  // npm's optional Next Playwright peer is devOptional; omitting dev alone
  // retains it. Production uses unoptimized images/plain mjs and needs neither
  // Playwright nor optional native build tooling. Build/test keeps full npm ci.
  const npm = path.join(path.dirname(process.execPath), "npm");
  const child = spawn(npm, ["ci", "--omit=dev", "--omit=optional", "--ignore-scripts", "--no-audit", "--no-fund"], {
    cwd: destination, detached: process.platform !== "win32", stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, NODE_ENV: "production" },
  });
  let output = 0;
  let killDeadline;
  const terminate = () => {
    try { if (process.platform === "win32") child.kill("SIGTERM"); else process.kill(-child.pid, "SIGTERM"); } catch {}
    killDeadline ??= setTimeout(() => {
      try { if (process.platform === "win32") child.kill("SIGKILL"); else process.kill(-child.pid, "SIGKILL"); } catch {}
    }, 2000);
  };
  const deadline = setTimeout(terminate, 120000);
  const aborted = () => terminate(); signal?.addEventListener("abort", aborted, { once: true });
  if (signal?.aborted) terminate();
  for (const stream of [child.stdout, child.stderr]) stream.on("data", (bytes) => { output += bytes.length; if (output > 1048576) terminate(); });
  try {
    await new Promise((resolve, reject) => { child.once("error", reject); child.once("close", (code) => code === 0 ? resolve() : reject(new Error("Locked production install failed"))); });
    signal?.throwIfAborted();
  } finally { clearTimeout(deadline); clearTimeout(killDeadline); signal?.removeEventListener("abort", aborted); }
}
export async function stageProductionRuntime({ sourceRoot, destination, signal }) {
  signal?.throwIfAborted();
  // Never overwrite another resource or reuse a developer node_modules tree.
  const inputs = JSON.parse(await readFile(path.join(sourceRoot, ".next/build-inputs.json"), "utf8"));
  if (JSON.stringify({ version: inputs.version, files: inputs.files, publicPresent: inputs.publicPresent }) !== JSON.stringify(await sourceBuildInputs(sourceRoot)) ||
      JSON.stringify(inputs.buildFiles) !== JSON.stringify(await buildOutputFiles(sourceRoot))) throw new Error("Stale production source/build custody");
  await mkdir(destination);
  try {
    for (const relative of topFiles) {
      if (["LICENSE", "NOTICE"].includes(relative)) await optionalCopy(path.join(sourceRoot, relative), path.join(destination, relative), signal);
      else await copyTree(path.join(sourceRoot, relative), path.join(destination, relative), signal);
    }
    await mkdir(path.join(destination, ".next"));
    for (const entry of (await readdir(path.join(sourceRoot, ".next"))).sort()) {
      if (buildRootFiles.has(entry) || entry === "build-inputs.json" || entry === "server" || entry === "static")
        await copyTree(path.join(sourceRoot, ".next", entry), path.join(destination, ".next", entry), signal);
    }
    await optionalCopy(path.join(sourceRoot, "public"), path.join(destination, "public"), signal);
    const sourceFiles = await inventory(destination);
    await install(destination, signal);
    const files = await inventory(destination);
    for (const [relative, hash] of Object.entries(sourceFiles)) {
      if (files[relative] !== hash || sha(await readFile(path.join(sourceRoot, relative))) !== hash)
        throw new Error("Source drift during staging");
    }
    const build = JSON.parse(await readFile(path.join(destination, ".next/required-server-files.json"), "utf8"));
    if (build.config.output || build.config.distDir !== ".next" ||
        JSON.stringify(build.config.pageExtensions) !== JSON.stringify(["public.tsx", "public.ts"])) throw new Error("Unsupported production build");
    if (JSON.stringify({ version: inputs.version, files: inputs.files, publicPresent: inputs.publicPresent }) !== JSON.stringify(await sourceBuildInputs(sourceRoot)) ||
        JSON.stringify(inputs.buildFiles) !== JSON.stringify(await buildOutputFiles(sourceRoot))) throw new Error("Source/build drift during staging");
    const manifest = { version: 1, files };
    await writeFile(path.join(destination, "runtime-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
    return manifest;
  } catch (error) { await rm(destination, { recursive: true, force: true }); throw error; }
}
export async function verifyProductionRuntime(destination) { return verifyRuntimeClosure(destination); }

async function sourceBuildInputs(root) {
  const files = {};
  async function collect(relative) {
    const target = path.join(root, relative); const metadata = await lstat(target);
    if (metadata.isSymbolicLink()) throw new Error("Build source link refused");
    if (metadata.isDirectory()) for (const entry of (await readdir(target)).sort()) await collect(`${relative}/${entry}`);
    else { if (!metadata.isFile()) throw new Error("Non-file build input"); files[relative] = sha(await readFile(target)); }
  }
  for (const relative of ["server.mjs", "next.config.mjs", "package.json", "package-lock.json", "tsconfig.json", "postcss.config.mjs", "tools/production-runtime.mjs", "src"]) await collect(relative);
  let publicPresent = false;
  try { await lstat(path.join(root, "public")); publicPresent = true; }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  if (publicPresent) await collect("public");
  return { version: 1, files, publicPresent };
}
async function buildProduction(root) {
  const before = await sourceBuildInputs(root);
  await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(root, "node_modules/next/dist/bin/next"), "build", "--webpack"], { cwd: root, stdio: "inherit", env: { ...process.env, NODE_ENV: "production" } });
    child.once("error", reject); child.once("close", (code) => code === 0 ? resolve() : reject(new Error("Production build failed")));
  });
  if (JSON.stringify(before) !== JSON.stringify(await sourceBuildInputs(root))) throw new Error("Source drift during build");
  await writeFile(path.join(root, ".next/build-inputs.json"), JSON.stringify({ ...before, buildFiles: await buildOutputFiles(root) }, null, 2) + "\n");
}
if (process.argv[1] && await realpath(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] === "--build") { await buildProduction(process.cwd()); }
  else {
  const [sourceRoot, destination] = process.argv.slice(2);
  if (!sourceRoot || !destination) throw new Error("usage: production-runtime.mjs SOURCE DESTINATION");
  await stageProductionRuntime({ sourceRoot: path.resolve(sourceRoot), destination: path.resolve(destination) });
  }
}
