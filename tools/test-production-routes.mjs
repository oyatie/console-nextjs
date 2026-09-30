import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { readFile, readdir } from "node:fs/promises";
import { createServer } from "node:net";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const expectedRoutes = [
  "/_global-error/page",
  "/_not-found/page",
  "/page",
  "/storefront/[id]/media/[mediaId]/route",
  "/storefront/[id]/page",
  "/storefront/page",
];
for (const file of [".next/server/app-paths-manifest.json", ".next/standalone/.next/server/app-paths-manifest.json"]) {
  const manifest = JSON.parse(await readFile(path.join(root, file), "utf8"));
  assert.deepEqual(Object.keys(manifest).sort(), expectedRoutes, `Unexpected production routes in ${file}`);
}
for (const directory of [".next/static/chunks", ".next/server", ".next/standalone/.next/server"]) {
  const absolute = path.join(root, directory);
  for (const file of await readdir(absolute, { recursive: true })) {
    assert.ok(!file.includes("(console)"), `Prototype chunk in ${directory}: ${file}`);
    if (!file.endsWith(".js")) continue;
    const content = await readFile(path.join(absolute, file), "utf8");
    for (const marker of ["박지영 수석", "AP-3121", "시연용 화면"]) {
      assert.ok(!content.includes(marker), `Prototype data in ${directory}: ${file}`);
    }
  }
}
const listener = createServer().listen(0, "127.0.0.1");
await once(listener, "listening");
const port = listener.address().port;
await new Promise((resolve, reject) => listener.close((error) => error ? reject(error) : resolve()));

const server = spawn(process.execPath, [path.join(root, ".next/standalone/server.js")], {
  cwd: root,
  env: { ...process.env, HOSTNAME: "127.0.0.1", PORT: String(port), NODE_ENV: "production", CONSOLE_BACKEND_ORIGIN: "" },
  stdio: ["ignore", "pipe", "pipe"],
});
let output = "";
for (const stream of [server.stdout, server.stderr]) {
  stream.on("data", (chunk) => { output += chunk.toString(); });
}

const base = `http://127.0.0.1:${port}`;
try {
  let ready = false;
  for (let attempt = 0; attempt < 60; attempt += 1) {
    if (server.exitCode !== null) throw new Error(`Production server exited:\n${output}`);
    try {
      await fetch(base, { redirect: "manual" });
      ready = true;
      break;
    } catch {
      await delay(250);
    }
  }
  assert.ok(ready, `Production server did not start:\n${output}`);

  const entry = await fetch(base, { redirect: "manual" });
  assert.ok([307, 308].includes(entry.status), `Root returned ${entry.status}`);
  const destination = new URL(entry.headers.get("location"), base);
  assert.equal(destination.pathname.replace(/\/$/, ""), "/storefront");

  const media = await fetch(`${base}/storefront/00000000-0000-4000-8000-000000000001/media/00000000-0000-4000-8000-000000000002/`);
  assert.equal(media.status, 503, `Media route returned ${media.status} without a backend`);

  const consoleDirectory = path.join(root, "src/app/(console)");
  const pages = (await readdir(consoleDirectory, { recursive: true }))
    .filter((file) => file.endsWith("/page.tsx"))
    .map((file) => `/${file.slice(0, -"/page.tsx".length)}`);
  assert.ok(pages.length > 0, "No console routes discovered");
  for (const page of pages) {
    for (const suffix of ["", "/"]) {
      const response = await fetch(`${base}${page}${suffix}`);
      assert.equal(response.status, 404, `${page}${suffix} returned ${response.status}`);
      assert.ok(!(await response.text()).includes("시연용 화면"), `${page}${suffix} rendered prototype UI`);
    }
  }
  console.log(`Production / redirects to /storefront; ${pages.length} console routes return 404; fixture chunks absent; media reports unavailable backend.`);
} finally {
  server.kill();
  if (server.exitCode === null && server.signalCode === null) {
    const force = setTimeout(() => server.kill("SIGKILL"), 2000);
    await once(server, "exit");
    clearTimeout(force);
  }
}
