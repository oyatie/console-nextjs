import { createServer as httpServer } from "node:http";
import { createServer as httpsServer } from "node:https";
import { lstat, readFile, readdir, readlink, realpath } from "node:fs/promises";
import { createHash, randomBytes } from "node:crypto";
import { isIP } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import next from "next";

// This listener is the sole owner of socket provenance. No caller forwarding
// field becomes authenticated ingress, including duplicate raw header fields.
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
export const topFiles = ["server.mjs", "next.config.mjs", "package.json", "package-lock.json", "LICENSE", "NOTICE"];
const devDependency = /^node_modules\/(?:@playwright\/test|playwright(?:-core)?|vitest|typescript|eslint)\//;
function allowed(relative) {
  return (topFiles.includes(relative) || relative.startsWith("public/") || relative.startsWith("node_modules/") ||
    relative.startsWith(".next/")) && !/(^|\/)\.env(?:\.|$)/.test(relative) && !devDependency.test(relative) &&
    !/^\.next\/(?:cache|types|dev|standalone)(?:\/|$)/.test(relative);
}
export async function runtimeInventory(root, prefix = "") {
  const files = {};
  for (const entry of (await readdir(path.join(root, prefix))).sort()) {
    const relative = prefix ? `${prefix}/${entry}` : entry;
    if (relative === "runtime-manifest.json") continue;
    const absolute = path.join(root, relative); const metadata = await lstat(absolute);
    if (metadata.isDirectory()) Object.assign(files, await runtimeInventory(root, relative));
    else {
      if (!allowed(relative)) throw new Error(`Unexpected runtime input: ${relative}`);
      if (metadata.isSymbolicLink()) {
        if (!relative.startsWith("node_modules/")) throw new Error("Source symlink refused");
        const dependencies = await realpath(path.join(root, "node_modules"));
        const link = await readlink(absolute);
        const immediate = path.resolve(await realpath(path.dirname(absolute)), link);
        const target = await realpath(absolute);
        if (!immediate.startsWith(`${dependencies}${path.sep}`) || !target.startsWith(`${dependencies}${path.sep}`) ||
            !(await lstat(target)).isFile()) throw new Error("Escaping dependency link");
        files[relative] = `link:${sha(Buffer.from(link))}`;
      } else {
        if (!metadata.isFile()) throw new Error("Non-file runtime input");
        files[relative] = sha(await readFile(absolute));
      }
    }
  }
  return files;
}
export async function verifyRuntimeClosure(destination) {
  if ((await lstat(path.join(destination, "runtime-manifest.json"))).isSymbolicLink()) throw new Error("Manifest symlink refused");
  const manifest = JSON.parse(await readFile(path.join(destination, "runtime-manifest.json"), "utf8"));
  if (manifest.version !== 1 || JSON.stringify(manifest.files) !== JSON.stringify(await runtimeInventory(destination))) throw new Error("Changed runtime closure");
  for (const relative of ["server.mjs", "next.config.mjs", "package.json", "package-lock.json", ".next/BUILD_ID", ".next/build-inputs.json", ".next/server/app-paths-manifest.json"])
    if (!manifest.files[relative]) throw new Error("Incomplete runtime closure");
  return manifest;
}

async function serve() {
process.env.NODE_ENV = "production";
process.env.CONSOLE_NODE_INGRESS_MARKER = randomBytes(32).toString("base64url");
const directory = path.dirname(fileURLToPath(import.meta.url));
const portText = process.env.PORT ?? "5173";
if (!/^[1-9][0-9]*$/.test(portText) || Number(portText) > 65535) throw new Error("Invalid listener port");
const port = Number(portText);
const hostname = process.env.HOSTNAME ?? "127.0.0.1";
const config = (await import("./next.config.mjs")).default;
const build = JSON.parse(await readFile(path.join(directory, ".next/required-server-files.json"), "utf8"));
if (config.distDir !== ".next" || config.output || build.config.output || build.config.distDir !== ".next" ||
    build.config.trailingSlash !== config.trailingSlash || build.config.images.unoptimized !== config.images.unoptimized ||
    JSON.stringify(build.config.pageExtensions) !== JSON.stringify(config.pageExtensions) ||
    JSON.stringify(config.pageExtensions) !== JSON.stringify(["public.tsx", "public.ts"]) ||
    build.config.typescript.ignoreBuildErrors !== false || !(await readFile(path.join(directory, ".next/BUILD_ID"), "utf8")).trim())
  throw new Error("Unsupported or inconsistent production build");
// Staged closures additionally bind every runtime byte. The source launcher
// still validates normal build/config identity above, without a live source link.
let staged = false;
try { await lstat(path.join(directory, "runtime-manifest.json")); staged = true; }
catch (error) { if (error.code !== "ENOENT") throw new Error("Invalid runtime closure"); }
if (staged) await verifyRuntimeClosure(directory);
const buildInputs = JSON.parse(await readFile(path.join(directory, ".next/build-inputs.json"), "utf8"));
if (buildInputs.version !== 1) throw new Error("Invalid build custody");
for (const relative of ["server.mjs", "next.config.mjs", "package.json", "package-lock.json"]) {
  if (sha(await readFile(path.join(directory, relative))) !== buildInputs.files[relative]) throw new Error("Stale production build input");
}
let publicOrigin;
try {
  const configured = process.env.CONSOLE_PUBLIC_ORIGIN;
  if (configured) {
    const url = new URL(configured);
    if (url.origin !== configured || url.username || url.password) throw new Error();
    publicOrigin = url;
  }
} catch { throw new Error("Invalid public origin"); }
const tlsCert = process.env.CONSOLE_TLS_CERT_FILE;
const tlsKey = process.env.CONSOLE_TLS_KEY_FILE;
if (Boolean(tlsCert) !== Boolean(tlsKey)) throw new Error("Incomplete TLS configuration");
const tls = tlsCert && tlsKey ? { cert: await readFile(tlsCert), key: await readFile(tlsKey) } : null;
if (publicOrigin?.protocol === "https:" && !tls) throw new Error("Public HTTPS requires direct TLS");
const app = next({ dev: false, dir: directory, hostname, port });
await app.prepare();
const handle = app.getRequestHandler();
const untrusted = (name) => name === "forwarded" || name === "x-real-ip" || name.startsWith("x-forwarded-") ||
  name.startsWith("x-console-node-");
const listener = async (request, response) => {
  let peer = request.socket.remoteAddress;
  if (peer?.startsWith("::ffff:") && isIP(peer.slice(7)) === 4) peer = peer.slice(7);
  if (!peer || !isIP(peer)) { response.writeHead(400).end(); return; }
  for (const name of Object.keys(request.headers)) if (untrusted(name.toLowerCase())) delete request.headers[name];
  const raw = [];
  for (let i = 0; i < request.rawHeaders.length; i += 2) {
    if (!untrusted(request.rawHeaders[i].toLowerCase())) raw.push(request.rawHeaders[i], request.rawHeaders[i + 1]);
  }
  const trusted = { "x-console-node-peer": peer, "x-console-node-secure": request.socket.encrypted ? "1" : "0",
    "x-console-node-ingress": process.env.CONSOLE_NODE_INGRESS_MARKER };
  for (const [name, value] of Object.entries(trusted)) { request.headers[name] = value; raw.push(name, value); }
  request.rawHeaders = raw;
  let pathname;
  try { pathname = new URL(request.url, "http://listener.invalid").pathname; }
  catch { response.writeHead(400).end(); return; }
  if (/^\/(?:login(?:\/|$)|me(?:\/|$)|api\/browser-session(?:\/|$))/.test(pathname)) {
    response.setHeader("Cache-Control", "private, no-store");
    response.setHeader("X-Content-Type-Options", "nosniff");
    const hosts = raw.filter((_, index) => index % 2 === 0 && raw[index].toLowerCase() === "host");
    const loopback = peer === "127.0.0.1" || peer === "::1";
    if (!publicOrigin || request.headers.host !== publicOrigin.host || hosts.length !== 1 ||
        (tls ? publicOrigin.protocol !== "https:" :
          publicOrigin.protocol !== "http:" || !["localhost", "127.0.0.1", "[::1]"].includes(publicOrigin.hostname) ||
          !loopback || process.env.CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP !== "true")) {
      response.writeHead(503).end("접근 설정을 확인할 수 없습니다."); return;
    }
  }
  try { await handle(request, response); }
  catch { if (!response.headersSent) response.writeHead(500); response.end(); }
};
const server = tls ? httpsServer(tls, listener) : httpServer(listener);
server.listen(port, hostname);
let stopping = false;
for (const signal of ["SIGTERM", "SIGINT"]) process.once(signal, () => {
  if (stopping) return; stopping = true;
  server.close(() => { void app.close().then(() => process.exit(0), () => process.exit(1)); });
  server.closeAllConnections();
});

}
if (process.argv[1] && await realpath(process.argv[1]) === fileURLToPath(import.meta.url)) await serve();
