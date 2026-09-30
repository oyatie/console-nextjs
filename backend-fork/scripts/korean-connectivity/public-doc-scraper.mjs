#!/usr/bin/env node
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { redactText } from "./evidence-ledger.mjs";
import { sha256, stableJson } from "./adapter-sdk.mjs";

const DEFAULT_MAX_BYTES = 128 * 1024;
const DEFAULT_TIMEOUT_MS = 10_000;
const DEFAULT_RATE_LIMIT_MS = 250;
const MAX_PUBLIC_DOC_BYTES = 1024 * 1024;
const MAX_TIMEOUT_MS = 60_000;
const MAX_RATE_LIMIT_MS = 60_000;

export function parseArgs(argv) {
  const args = {
    catalog: "docs/benchmarks/korean-institutional-connectivity-catalog.fixture.json",
    out: ".tmp/korean-connectivity-public-doc-cache.json",
    allowPublicFetch: false,
    maxBytes: DEFAULT_MAX_BYTES,
    timeoutMs: DEFAULT_TIMEOUT_MS,
    rateLimitMs: DEFAULT_RATE_LIMIT_MS,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--catalog") args.catalog = argv[++i];
    else if (arg === "--out") args.out = argv[++i];
    else if (arg === "--allow-public-fetch") args.allowPublicFetch = true;
    else if (arg === "--offline") args.allowPublicFetch = false;
    else if (arg === "--max-bytes") args.maxBytes = Number(argv[++i]);
    else if (arg === "--timeout-ms") args.timeoutMs = Number(argv[++i]);
    else if (arg === "--rate-limit-ms") args.rateLimitMs = Number(argv[++i]);
    else throw new Error(`unknown argument: ${arg}`);
  }
  return args;
}

function assertBoundedInteger(name, value, { min = 0, max = Number.MAX_SAFE_INTEGER } = {}) {
  if (!Number.isSafeInteger(value) || value < min || value > max) {
    throw new Error(`${name} must be a safe integer between ${min} and ${max}`);
  }
  return value;
}

export function validatePublicDocFetchOptions({ maxBytes = DEFAULT_MAX_BYTES, timeoutMs = DEFAULT_TIMEOUT_MS, rateLimitMs = DEFAULT_RATE_LIMIT_MS } = {}) {
  return {
    maxBytes: assertBoundedInteger("maxBytes", maxBytes, { min: 1, max: MAX_PUBLIC_DOC_BYTES }),
    timeoutMs: assertBoundedInteger("timeoutMs", timeoutMs, { min: 1, max: MAX_TIMEOUT_MS }),
    rateLimitMs: assertBoundedInteger("rateLimitMs", rateLimitMs, { min: 0, max: MAX_RATE_LIMIT_MS }),
  };
}

export function isPublicFetchUrlAllowed(url) {
  if (typeof url !== "string") return false;
  if (!url.startsWith("https://")) return false;
  const lowered = url.toLowerCase();
  return !["login", "logon", "signin", "auth", "certificate", "cert", "keyboard", "mfa", "otp", "session"].some((forbidden) => lowered.includes(forbidden));
}

export function isFetchAllowed(source) {
  if (!source || typeof source.url !== "string") return false;
  if (!isPublicFetchUrlAllowed(source.url)) return false;
  if (source.scraping_allowed_scope === "no_fetch_manual_reference") return false;
  if (!["public_metadata_only", "official_sandbox_metadata_only"].includes(source.scraping_allowed_scope)) return false;
  if (!["official_docs", "openapi_spec", "static_html_doc", "file_format_spec", "sandbox_reference"].includes(source.source_type)) return false;
  return true;
}

export function buildSourceManifest(catalog) {
  const records = [];
  for (const connector of catalog.connectors ?? []) {
    for (const source of connector.source_evidence ?? []) {
      records.push({
        connector_id: connector.institution_id,
        capability_state: connector.capability_state,
        legal_status: connector.legal_status,
        url: source.url,
        source_type: source.source_type,
        fetched_at_policy: source.fetched_at_policy,
        license_or_terms_status: source.license_or_terms_status,
        scraping_allowed_scope: source.scraping_allowed_scope,
        public_fetch_allowed: isFetchAllowed(source),
      });
    }
  }
  return records.sort((a, b) => `${a.connector_id}:${a.url}`.localeCompare(`${b.connector_id}:${b.url}`));
}

async function readBoundedResponseText(response, maxBytes) {
  if (!response.body?.getReader) {
    throw new Error("response body stream is required for bounded public-doc fetch");
  }
  const reader = response.body.getReader();
  const chunks = [];
  let bytesRead = 0;
  let reachedCap = false;
  try {
    while (!reachedCap) {
      const { done, value } = await reader.read();
      if (done) break;
      const chunk = Buffer.from(value);
      const remaining = maxBytes - bytesRead;
      if (remaining <= 0) {
        reachedCap = true;
        break;
      }
      chunks.push(chunk.subarray(0, remaining));
      bytesRead += Math.min(chunk.byteLength, remaining);
      reachedCap = bytesRead >= maxBytes;
    }
    if (reachedCap) {
      await reader.cancel();
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function fetchPublicMetadata(record, { maxBytes, timeoutMs }) {
  if (!record.public_fetch_allowed) {
    return {
      ...record,
      fetch_mode: "blocked_by_policy",
      blocked_reason: "source is not public-fetch eligible",
    };
  }
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(record.url, {
      method: "GET",
      redirect: "manual",
      signal: controller.signal,
      headers: {
        "accept": "text/html,application/json,application/yaml,text/plain;q=0.8,*/*;q=0.1",
        "user-agent": "maintenance-korean-connectivity-public-doc-cache/fixture-foundation",
      },
    });
    if (response.status >= 300 && response.status < 400) {
      const location = response.headers.get("location") ?? "";
      return {
        ...record,
        fetch_mode: "blocked_by_policy",
        blocked_reason: "redirects require explicit source reclassification before body fetch",
        status: response.status,
        redirect_location_hash: location ? sha256(location) : null,
      };
    }
    const finalUrl = response.url || record.url;
    if (!isPublicFetchUrlAllowed(finalUrl)) {
      return {
        ...record,
        fetch_mode: "blocked_by_policy",
        blocked_reason: "final URL is not public-fetch eligible",
        status: response.status,
        final_url_hash: sha256(finalUrl),
      };
    }
    const contentType = response.headers.get("content-type") ?? "";
    const clipped = await readBoundedResponseText(response, maxBytes);
    const redacted = redactText(clipped);
    return {
      ...record,
      fetch_mode: "public_read_only_fetch",
      status: response.status,
      content_type: contentType,
      bytes_read: Buffer.byteLength(clipped),
      body_sha256: sha256(clipped),
      redacted_excerpt: redacted.text.slice(0, 1000),
      redaction_findings: redacted.findings.map(({ rule_id, label }) => ({ rule_id, label })),
    };
  } finally {
    clearTimeout(timeout);
  }
}

function sleep(ms) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

export async function cachePublicDocMetadata({ catalogPath, outPath, allowPublicFetch = false, maxBytes = DEFAULT_MAX_BYTES, timeoutMs = DEFAULT_TIMEOUT_MS, rateLimitMs = DEFAULT_RATE_LIMIT_MS }) {
  const limits = validatePublicDocFetchOptions({ maxBytes, timeoutMs, rateLimitMs });
  const catalog = JSON.parse(await readFile(resolve(catalogPath), "utf8"));
  const manifest = buildSourceManifest(catalog);
  const records = [];
  for (const [index, source] of manifest.entries()) {
    if (allowPublicFetch) {
      if (index > 0 && limits.rateLimitMs > 0) await sleep(limits.rateLimitMs);
      records.push(await fetchPublicMetadata(source, { maxBytes: limits.maxBytes, timeoutMs: limits.timeoutMs }));
    } else {
      records.push({
        ...source,
        fetch_mode: "metadata_only_no_network",
        body_sha256: sha256(source.url),
      });
    }
  }
  const cache = {
    version: 1,
    generated_at: allowPublicFetch ? new Date().toISOString() : "fixture-time",
    policy: {
      allow_public_fetch: allowPublicFetch,
      max_bytes: limits.maxBytes,
      timeout_ms: limits.timeoutMs,
      rate_limit_ms: limits.rateLimitMs,
      forbidden: [
        "login-required pages",
        "customer portals",
        "certificate dialogs",
        "production sessions",
        "session cookies",
        "browser storage",
        "institution security-plugin internals",
        "anti-automation bypasses",
      ],
    },
    records,
    manifest_hash: sha256(records.map(({ connector_id, url, scraping_allowed_scope }) => ({ connector_id, url, scraping_allowed_scope }))),
  };
  await mkdir(dirname(resolve(outPath)), { recursive: true });
  await writeFile(resolve(outPath), `${stableJson(cache)}\n`, "utf8");
  return cache;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const args = parseArgs(process.argv.slice(2));
  const cache = await cachePublicDocMetadata({
    catalogPath: args.catalog,
    outPath: args.out,
    allowPublicFetch: args.allowPublicFetch,
    maxBytes: args.maxBytes,
    timeoutMs: args.timeoutMs,
    rateLimitMs: args.rateLimitMs,
  });
  console.log(JSON.stringify({ ok: true, out: args.out, records: cache.records.length, allowPublicFetch: args.allowPublicFetch }, null, 2));
}
