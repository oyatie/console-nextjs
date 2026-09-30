#!/usr/bin/env node
import { generateKeyPairSync } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const failures = [];
const passes = [];

function pathOf(path) {
  return resolve(root, path);
}

function read(path) {
  const abs = pathOf(path);
  if (!existsSync(abs)) {
    failures.push(`${path}: missing`);
    return "";
  }
  return readFileSync(abs, "utf8");
}

function json(path) {
  try {
    return JSON.parse(read(path));
  } catch (error) {
    failures.push(`${path}: invalid JSON: ${error.message}`);
    return {};
  }
}

function requireFile(path, label = path) {
  if (existsSync(pathOf(path))) {
    passes.push(`${label}: present`);
  } else {
    failures.push(`${label}: missing (${path})`);
  }
}

function requireIncludes(path, needle, label) {
  if (read(path).includes(needle)) {
    passes.push(label);
  } else {
    failures.push(`${label}: ${path} must include ${JSON.stringify(needle)}`);
  }
}

function requireNotIncludes(path, needle, label) {
  if (!read(path).includes(needle)) {
    passes.push(label);
  } else {
    failures.push(`${label}: ${path} must not include ${JSON.stringify(needle)}`);
  }
}

function require(condition, label, failure) {
  if (condition) {
    passes.push(label);
  } else {
    failures.push(failure ?? label);
  }
}

const adr = "docs/decisions/ADR-0020-korean-institutional-connectivity-coverage-factory.md";
const spec = "docs/specs/korean-institutional-connectivity.md";
const schemaPath = "docs/benchmarks/korean-institutional-connectivity-catalog.schema.json";
const fixturePath = "docs/benchmarks/korean-institutional-connectivity-catalog.fixture.json";
const adapterSdkPath = "scripts/korean-connectivity/adapter-sdk.mjs";
const adapterEchoFixturePath = "docs/benchmarks/fixtures/korean-connectivity/adapter-sdk-echo.fixture.json";
const evidenceLedgerPath = "scripts/korean-connectivity/evidence-ledger.mjs";
const evidenceRedactionFixturePath = "docs/benchmarks/fixtures/korean-connectivity/evidence-redaction.fixture.json";
const publicDocScraperPath = "scripts/korean-connectivity/public-doc-scraper.mjs";
const openBankingFixturePath = "scripts/korean-connectivity/open-banking-fixture.mjs";
const openBankingFixtureDataPath = "docs/benchmarks/fixtures/korean-connectivity/kftc-open-banking.fixture.json";
const localAgentSimulatorPath = "scripts/korean-connectivity/local-agent-simulator.mjs";
const localAgentSimulatorFixturePath = "docs/benchmarks/fixtures/korean-connectivity/local-agent-simulator.fixture.json";
const localCertificateAgentPath = "scripts/korean-connectivity/local-certificate-agent.mjs";
const localCertificateAgentFixturePath = "docs/benchmarks/fixtures/korean-connectivity/local-certificate-agent.fixture.json";
const localCertificateAgentSpecPath = "docs/specs/korean-institutional-connectivity-local-certificate-agent.md";
const nhisLossReportPath = "scripts/korean-connectivity/nhis-edi-loss-report.mjs";
const nhisLossReportFixturePath = "docs/benchmarks/fixtures/korean-connectivity/nhis-edi-loss-report.fixture.json";
const nhisLossReportSpecPath = "docs/specs/korean-institutional-connectivity-nhis-edi-loss-report.md";

for (const file of [adr, spec, schemaPath, fixturePath, adapterSdkPath, adapterEchoFixturePath, evidenceLedgerPath, evidenceRedactionFixturePath, publicDocScraperPath, openBankingFixturePath, openBankingFixtureDataPath, localAgentSimulatorPath, localAgentSimulatorFixturePath, localCertificateAgentPath, localCertificateAgentFixturePath, localCertificateAgentSpecPath, nhisLossReportPath, nhisLossReportFixturePath, nhisLossReportSpecPath]) {
  requireFile(file);
}

for (const phrase of [
  "Do not use CODEF, Popbill, Tilko/Tilkoblet, or similar aggregators as runtime dependencies.",
  "Production must not centrally store or operate raw customer 공동인증서/private keys, certificate passwords, bank passwords, or unmanaged financial credentials.",
  "Do not proxy, intercept, replay, or bypass customer browser sessions",
  "No live filing, payment, production signing, production institution login, or production institution API call",
  "Public official API/documentation scraping is allowed only as source discovery",
]) {
  requireIncludes(adr, phrase, `ADR invariant: ${phrase.slice(0, 64)}`);
}

for (const phrase of [
  "Allowed scraping scope is narrow and public",
  "Forbidden scraping scope includes authenticated customer portals",
  "Allowed data crossing from local-agent/customer session to platform",
  "Forbidden data crossing includes 공동인증서/private keys",
  "Local certificate signing must happen in a customer-controlled local agent",
  "The platform receives signed login proof envelopes only",
  "The platform must not proxy, intercept, replay, or bypass customer browser sessions",
  "research_only -> fixture_only",
  "Any state can move to `prohibited`",
]) {
  requireIncludes(spec, phrase, `spec boundary: ${phrase.slice(0, 64)}`);
}

requireIncludes("package.json", "check:korean-institutional-connectivity", "package script is wired");
requireIncludes("package.json", "kic:scrape-public-docs", "public doc scraper script is wired");
requireIncludes("package.json", "kic:generate-nhis-loss-report-fixture", "NHIS loss report generator script is wired");
requireIncludes("package.json", "kic:local-cert-login-fixture", "local certificate login fixture script is wired");
requireNotIncludes("package.json", "codef", "package has no lowercase codef dependency token");
requireNotIncludes("package.json", "popbill", "package has no popbill dependency token");
requireNotIncludes("package.json", "tilko", "package has no tilko dependency token");

const schema = json(schemaPath);
const fixture = json(fixturePath);
const states = ["research_only", "fixture_only", "sandbox", "partner_approved", "live_read", "live_write", "prohibited", "deprecated"];
const authModes = ["official_oauth", "mydata_token", "nts_asp_certified", "local_agent_cert_session", "manual_file_upload", "fixture_only"];
const sideEffects = ["read_only", "generated_file", "human_approved_filing", "automated_filing", "payment_transfer", "certified_issuance"];

const schemaText = JSON.stringify(schema);
for (const state of states) {
  require(schemaText.includes(state), `schema state ${state}`, `schema must include state ${state}`);
}
for (const mode of authModes) {
  require(schemaText.includes(mode), `schema auth mode ${mode}`, `schema must include auth mode ${mode}`);
}
for (const sideEffect of sideEffects) {
  require(schemaText.includes(sideEffect), `schema side-effect ${sideEffect}`, `schema must include side-effect ${sideEffect}`);
}
require(schemaText.includes("source_evidence"), "schema requires source evidence", "schema must include source_evidence");
require(schemaText.includes("scraping_allowed_scope"), "schema constrains scraping scope", "schema must include scraping_allowed_scope");
require(schemaText.includes("forbidden_data"), "schema requires forbidden data list", "schema must include forbidden_data");

const fixtureStates = fixture?.state_machine?.states ?? [];
require(states.every((state) => fixtureStates.includes(state)), "fixture state machine includes all states", "fixture state_machine.states missing required state");
require(fixture?.state_machine?.default_terminal_state === "prohibited", "fixture default terminal state prohibited", "fixture default terminal state must be prohibited");

const connectors = fixture?.connectors ?? [];
require(connectors.length >= 5, "fixture has five initial catalog exemplars", "fixture must include at least five connectors");
const ids = new Set(connectors.map((connector) => connector.institution_id));
for (const id of ["kftc_open_banking", "financial_mydata_standard_api", "nhis_edi_loss_report", "nts_asp_einvoice_readiness", "local_agent_cert_session_simulator"]) {
  require(ids.has(id), `fixture connector ${id}`, `fixture missing connector ${id}`);
}

for (const connector of connectors) {
  const label = connector.institution_id ?? "<missing institution_id>";
  require(states.includes(connector.capability_state), `${label}: valid capability state`, `${label}: invalid capability state`);
  require(Array.isArray(connector.source_evidence) && connector.source_evidence.length > 0, `${label}: source evidence present`, `${label}: missing source evidence`);
  require(Array.isArray(connector.forbidden_data) && connector.forbidden_data.length > 0, `${label}: forbidden data present`, `${label}: missing forbidden data`);
  require(connector.fixture_path?.startsWith("docs/benchmarks/fixtures/"), `${label}: fixture path is repo-local`, `${label}: fixture_path must be repo-local fixture path`);
  require(connector.evidence_policy?.source_urls === true, `${label}: source URLs evidence enabled`, `${label}: evidence_policy.source_urls must be true`);
  for (const evidence of connector.source_evidence ?? []) {
    require(typeof evidence.url === "string" && evidence.url.startsWith("https://"), `${label}: source evidence URL is https`, `${label}: source evidence URL must be https`);
    require(["public_metadata_only", "no_fetch_manual_reference", "official_sandbox_metadata_only"].includes(evidence.scraping_allowed_scope), `${label}: scraping scope is constrained`, `${label}: scraping scope is unconstrained`);
  }
}


const adapterSdk = await import("./korean-connectivity/adapter-sdk.mjs");
for (const exportName of ["ConnectivityPolicyError", "createFixtureConnector", "executeFixtureWorkflow", "withNoLiveNetworkGuard", "prepareIntent", "sha256"]) {
  require(typeof adapterSdk[exportName] !== "undefined", `adapter SDK export ${exportName}`, `adapter SDK missing export ${exportName}`);
}
const fixtureConnector = adapterSdk.createFixtureConnector({
  connector_id: "adapter_sdk_echo",
  side_effect_class: "read_only",
  fixture_path: adapterEchoFixturePath,
  source_urls: ["https://openapi.kftc.or.kr/service/openBanking"],
  workflows: [
    {
      workflow_id: "fixture.echo",
      side_effect_class: "read_only",
      fixture_path: adapterEchoFixturePath,
    },
  ],
});
const fixtureRunA = await adapterSdk.executeFixtureWorkflow(fixtureConnector, "fixture.echo", { b: 2, a: 1 });
const fixtureRunB = await adapterSdk.executeFixtureWorkflow(fixtureConnector, "fixture.echo", { a: 1, b: 2 });
require(fixtureRunA.execution_mode === "fixture_only", "adapter SDK returns fixture_only execution mode", "adapter SDK must return fixture_only execution mode");
require(fixtureRunA.intent_hash === fixtureRunB.intent_hash, "adapter SDK intent hash is deterministic", "adapter SDK intent hash must be deterministic for stable JSON payloads");
require(fixtureRunA.evidence?.deterministic_fixture_hash === fixtureRunB.evidence?.deterministic_fixture_hash, "adapter SDK fixture hash is deterministic", "adapter SDK fixture hash must be deterministic");
let blockedFetch = false;
await adapterSdk.withNoLiveNetworkGuard(async () => {
  try {
    await fetch("https://example.invalid/should-not-run");
  } catch (error) {
    blockedFetch = error?.name === "ConnectivityPolicyError";
  }
});
require(blockedFetch, "adapter SDK blocks live fetch in fixture guard", "adapter SDK fixture guard must block live fetch");
let blockedEndpoint = false;
try {
  adapterSdk.createFixtureConnector({
    connector_id: "bad_live_endpoint",
    execution_mode: "fixture_only",
    live_endpoint: "https://bank.example.invalid/live",
    workflows: [{ workflow_id: "bad", fixture_path: adapterEchoFixturePath }],
  });
} catch (error) {
  blockedEndpoint = error?.name === "ConnectivityPolicyError";
}
require(blockedEndpoint, "adapter SDK rejects live endpoint", "adapter SDK must reject live endpoints in fixture mode");

const evidenceLedger = await import("./korean-connectivity/evidence-ledger.mjs");
for (const exportName of ["redactText", "redactJson", "createEvidenceRecord", "assertEvidenceSafe", "appendEvidenceRecord"]) {
  require(typeof evidenceLedger[exportName] !== "undefined", `evidence ledger export ${exportName}`, `evidence ledger missing export ${exportName}`);
}
const redactionFixture = json(evidenceRedactionFixturePath);
const redactedTranscript = evidenceLedger.redactText(`${redactionFixture.transcript}\n${JSON.stringify(redactionFixture.output)}`);
for (const expected of redactionFixture.expected_redactions ?? []) {
  require(redactedTranscript.findings.some((finding) => finding.rule_id === expected), `redaction finding ${expected}`, `expected redaction finding ${expected}`);
}
const evidenceRecord = evidenceLedger.createEvidenceRecord({
  connectorId: "nhis_edi_loss_report",
  workflowId: "social_insurance.loss_report.generate_file",
  intentHash: fixtureRunA.intent_hash,
  parserVersion: "fixture-parser-v1",
  sourceUrls: ["https://edi.nhis.or.kr/webedi/file_sy/all_sangsil.html"],
  transcript: redactionFixture.transcript,
  output: redactionFixture.output,
  observedAt: "2026-07-03T00:00:00Z",
});
require(evidenceLedger.assertEvidenceSafe(evidenceRecord) === true, "evidence record passes safety assertion", "evidence record should pass safety assertion");
const evidenceSerialized = JSON.stringify(evidenceRecord);
for (const forbidden of ["991231-1234567", "123-456-789012", "SESSION=fixture", "fixture-secret", "signPri.key", "bank_password=do-not-store"]) {
  require(!evidenceSerialized.includes(forbidden), `evidence redacts ${forbidden}`, `evidence record leaked ${forbidden}`);
}
require(evidenceRecord.source_urls.includes("https://edi.nhis.or.kr/webedi/file_sy/all_sangsil.html"), "evidence record preserves source URL", "evidence record must preserve source URL");
require(evidenceRecord.redaction_findings.length >= 7, "evidence record stores redaction findings", "evidence record should store redaction findings");

const publicDocScraper = await import("./korean-connectivity/public-doc-scraper.mjs");
for (const exportName of ["buildSourceManifest", "cachePublicDocMetadata", "isFetchAllowed", "isPublicFetchUrlAllowed", "parseArgs", "validatePublicDocFetchOptions"]) {
  require(typeof publicDocScraper[exportName] !== "undefined", `public doc scraper export ${exportName}`, `public doc scraper missing export ${exportName}`);
}
const parsedScraperArgs = publicDocScraper.parseArgs(["--rate-limit-ms", "7", "--timeout-ms", "1234", "--max-bytes", "99"]);
require(parsedScraperArgs.rateLimitMs === 7, "public doc scraper parses rate-limit", "public doc scraper must parse rate-limit-ms");
require(parsedScraperArgs.timeoutMs === 1234, "public doc scraper parses timeout", "public doc scraper must parse timeout-ms");
require(parsedScraperArgs.maxBytes === 99, "public doc scraper parses max-bytes", "public doc scraper must parse max-bytes");
require(publicDocScraper.validatePublicDocFetchOptions({ maxBytes: 99, timeoutMs: 1234, rateLimitMs: 7 }).maxBytes === 99, "public doc scraper validates good numeric limits", "public doc scraper should validate good numeric limits");
for (const badMaxBytes of [0, -1, 1.5, Infinity, NaN, 1024 * 1024 + 1]) {
  let rejectedBadMaxBytes = false;
  try {
    publicDocScraper.validatePublicDocFetchOptions({ maxBytes: badMaxBytes, timeoutMs: 1234, rateLimitMs: 0 });
  } catch (error) {
    rejectedBadMaxBytes = /maxBytes must be a safe integer/.test(error.message);
  }
  require(rejectedBadMaxBytes, `public doc scraper rejects bad maxBytes ${String(badMaxBytes)}`, `public doc scraper must reject bad maxBytes ${String(badMaxBytes)}`);
}
const sourceManifest = publicDocScraper.buildSourceManifest(fixture);
const sourceEvidenceCount = connectors.reduce((sum, connector) => sum + (connector.source_evidence?.length ?? 0), 0);
require(sourceManifest.length === sourceEvidenceCount, "public doc scraper manifests every source", "public doc scraper must manifest every source evidence entry");
require(sourceManifest.some((source) => source.public_fetch_allowed === false && source.scraping_allowed_scope === "no_fetch_manual_reference"), "public doc scraper blocks manual legal references", "manual legal references must not be public-fetched");
require(sourceManifest.every((source) => source.url.startsWith("https://")), "public doc scraper only has https sources", "public doc scraper sources must be https");
require(publicDocScraper.isPublicFetchUrlAllowed("https://openapi.kftc.or.kr/service/openBanking") === true, "public doc scraper allows official public URL", "public doc scraper should allow official public URL");
require(publicDocScraper.isPublicFetchUrlAllowed("https://example.go.kr/login") === false, "public doc scraper blocks login URL", "public doc scraper must block login URL");
const publicDocCache = await publicDocScraper.cachePublicDocMetadata({
  catalogPath: fixturePath,
  outPath: ".tmp/korean-connectivity-public-doc-cache.check.json",
  allowPublicFetch: false,
  rateLimitMs: 0,
});
require(publicDocCache.records.length === sourceEvidenceCount, "public doc cache records every source", "public doc cache record count mismatch");
require(publicDocCache.records.every((record) => record.fetch_mode === "metadata_only_no_network"), "public doc cache default is no-network", "public doc cache should default to no-network metadata mode");
require(publicDocCache.records.every((record) => !Object.hasOwn(record, "raw_body")), "public doc cache stores no raw body", "public doc cache must not store raw_body");
require(publicDocCache.policy.forbidden.includes("customer portals"), "public doc cache records forbidden customer portals", "public doc cache policy must forbid customer portals");
require(publicDocCache.policy.rate_limit_ms === 0, "public doc cache records rate limit", "public doc cache policy must record rate_limit_ms");
require(publicDocCache.manifest_hash?.length === 64, "public doc cache has deterministic manifest hash", "public doc cache manifest hash missing");

const originalFetchForPublicDocs = globalThis.fetch;
let fetchCalledForBadMaxBytes = false;
globalThis.fetch = async () => {
  fetchCalledForBadMaxBytes = true;
  throw new Error("fetch should not run for invalid maxBytes");
};
let badMaxBytesBlockedBeforeFetch = false;
try {
  await publicDocScraper.cachePublicDocMetadata({
    catalogPath: fixturePath,
    outPath: ".tmp/korean-connectivity-public-doc-cache.bad-max-bytes-check.json",
    allowPublicFetch: true,
    maxBytes: Infinity,
    rateLimitMs: 0,
  });
} catch (error) {
  badMaxBytesBlockedBeforeFetch = /maxBytes must be a safe integer/.test(error.message);
}
require(badMaxBytesBlockedBeforeFetch, "public doc scraper rejects invalid maxBytes before fetch", "public doc scraper must reject invalid maxBytes before fetch");
require(fetchCalledForBadMaxBytes === false, "public doc scraper does not fetch after invalid maxBytes", "public doc scraper must not fetch after invalid maxBytes");
globalThis.fetch = async () => ({
  status: 302,
  url: "https://public.example.go.kr/source",
  headers: new Map([["location", "https://public.example.go.kr/login"]]),
  body: new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("SHOULD_NOT_BE_READ"));
      controller.close();
    },
  }),
});
const redirectCache = await publicDocScraper.cachePublicDocMetadata({
  catalogPath: fixturePath,
  outPath: ".tmp/korean-connectivity-public-doc-cache.redirect-check.json",
  allowPublicFetch: true,
  rateLimitMs: 0,
});
require(redirectCache.records.some((record) => record.fetch_mode === "blocked_by_policy" && record.blocked_reason.includes("redirects require explicit source reclassification")), "public doc scraper blocks redirects before body read", "public doc scraper must block redirects before body read");
globalThis.fetch = async () => ({
  status: 200,
  url: "https://public.example.go.kr/source",
  headers: new Map([["content-type", "text/html"]]),
  body: new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("A".repeat(128)));
      controller.close();
    },
  }),
});
const boundedCache = await publicDocScraper.cachePublicDocMetadata({
  catalogPath: fixturePath,
  outPath: ".tmp/korean-connectivity-public-doc-cache.bounded-check.json",
  allowPublicFetch: true,
  maxBytes: 16,
  rateLimitMs: 0,
});
const fetchedBounded = boundedCache.records.find((record) => record.fetch_mode === "public_read_only_fetch");
require(fetchedBounded?.bytes_read === 16, "public doc scraper enforces streaming byte cap", "public doc scraper must enforce streaming byte cap");
require(fetchedBounded?.redacted_excerpt.length === 16, "public doc scraper stores bounded excerpt only", "public doc scraper should store bounded excerpt only");
let exactCapCancelCount = 0;
globalThis.fetch = async () => ({
  status: 200,
  url: "https://public.example.go.kr/source",
  headers: new Map([["content-type", "text/html"]]),
  body: new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("B".repeat(16)));
    },
    cancel() {
      exactCapCancelCount += 1;
    },
  }),
});
const exactCapCache = await publicDocScraper.cachePublicDocMetadata({
  catalogPath: fixturePath,
  outPath: ".tmp/korean-connectivity-public-doc-cache.exact-cap-check.json",
  allowPublicFetch: true,
  maxBytes: 16,
  rateLimitMs: 0,
});
require(exactCapCache.records.some((record) => record.fetch_mode === "public_read_only_fetch" && record.bytes_read === 16), "public doc scraper reads exact cap", "public doc scraper should read exact maxBytes cap");
require(exactCapCancelCount > 0, "public doc scraper cancels exact-cap stream", "public doc scraper must cancel exact-cap stream");
globalThis.fetch = async () => ({
  status: 200,
  url: "https://public.example.go.kr/source",
  headers: new Map([["content-type", "text/html"]]),
  body: null,
});
let noBodyBlocked = false;
try {
  await publicDocScraper.cachePublicDocMetadata({
    catalogPath: fixturePath,
    outPath: ".tmp/korean-connectivity-public-doc-cache.no-body-check.json",
    allowPublicFetch: true,
    maxBytes: 16,
    rateLimitMs: 0,
  });
} catch (error) {
  noBodyBlocked = /response body stream is required/.test(error.message);
}
require(noBodyBlocked, "public doc scraper rejects unbounded no-body fallback", "public doc scraper must reject unbounded no-body fallback");
globalThis.fetch = originalFetchForPublicDocs;

const openBankingFixture = await import("./korean-connectivity/open-banking-fixture.mjs");
for (const exportName of ["OPEN_BANKING_SOURCE_URLS", "createOpenBankingFixtureConnector", "runOpenBankingFixture"]) {
  require(typeof openBankingFixture[exportName] !== "undefined", `open banking fixture export ${exportName}`, `open banking fixture missing export ${exportName}`);
}
const openBankingConnector = openBankingFixture.createOpenBankingFixtureConnector();
require(openBankingConnector.auth_shape?.mode === "official_oauth", "open banking connector uses official OAuth shape", "open banking connector must use official OAuth shape");
require(openBankingConnector.execution_mode === "fixture_only", "open banking connector is fixture-only", "open banking connector must be fixture-only");
const openBankingAccounts = await openBankingFixture.runOpenBankingFixture("banking.accounts.list", { fintech_use_num: "fixture-use-num" });
const openBankingTransactions = await openBankingFixture.runOpenBankingFixture("banking.transactions.list", { fintech_use_num: "fixture-use-num" });
require(openBankingAccounts.output.accounts.length === 1, "open banking fixture returns one account", "open banking accounts fixture should return one account");
require(openBankingTransactions.output.transactions.length === 2, "open banking fixture returns two transactions", "open banking transactions fixture should return two transactions");
require(openBankingAccounts.evidence_record.source_urls.includes("https://openapi.kftc.or.kr/service/openBanking"), "open banking evidence includes KFTC portal source", "open banking evidence must include KFTC portal source");
const openBankingCombined = JSON.stringify({ openBankingAccounts, openBankingTransactions, fixture: json(openBankingFixtureDataPath) });
for (const forbidden of ["access_token", "refresh_token", "bank_password", "security_card", "session_cookie"]) {
  require(!openBankingCombined.includes(`\"${forbidden}\":`), `open banking fixture has no ${forbidden} field`, `open banking fixture must not contain ${forbidden} field`);
}
require(!/\b\d{2,6}-\d{2,6}-\d{2,8}\b/.test(openBankingCombined), "open banking fixture has no unmasked account pattern", "open banking fixture must not expose full account pattern");

const localAgentSimulator = await import("./korean-connectivity/local-agent-simulator.mjs");
for (const exportName of ["createLocalAgentIntent", "simulateLocalAgentSignature", "assertAllowedLocalAgentOutput", "runLocalAgentFixture"]) {
  require(typeof localAgentSimulator[exportName] !== "undefined", `local agent simulator export ${exportName}`, `local agent simulator missing export ${exportName}`);
}
const localAgentRun = localAgentSimulator.runLocalAgentFixture({ payload: { filing_kind: "loss_report_fixture" } });
require(localAgentRun.proof.proof_type === "fixture_signed_challenge", "local agent returns fixture signed challenge", "local agent should return fixture signed challenge");
require(localAgentRun.proof.signature_algorithm === "fixture_hmac_sha256_not_a_certificate", "local agent signature is explicitly non-certificate", "local agent signature must be marked fixture/non-certificate");
require(localAgentRun.proof.attestation.real_certificate_used === false, "local agent fixture uses no real certificate", "local agent fixture must not use real certificate");
require(localAgentRun.proof.attestation.private_key_exported === false, "local agent fixture exports no private key", "local agent fixture must not export private key");
require(localAgentRun.evidence_record.source_urls.some((url) => url.includes("law.go.kr")), "local agent evidence cites legal source", "local agent evidence must cite legal source");
let blockedLocalAgentOutput = false;
try {
  localAgentSimulator.assertAllowedLocalAgentOutput({ signed_challenge: "ok", certPassword: "bad" });
} catch (error) {
  blockedLocalAgentOutput = /forbidden local-agent output key/.test(error.message);
}
require(blockedLocalAgentOutput, "local agent blocks forbidden cert password output", "local agent must block forbidden cert password output");
const localAgentCombined = JSON.stringify({ run: localAgentRun, fixture: json(localAgentSimulatorFixturePath) });
for (const forbidden of ["BEGIN PRIVATE KEY", "pfx=", "signPri.key uploaded", "session_cookie=", "localStorage:"]) {
  require(!localAgentCombined.includes(forbidden), `local agent fixture avoids ${forbidden}`, `local agent fixture leaked ${forbidden}`);
}

const localCertificateAgent = await import("./korean-connectivity/local-certificate-agent.mjs");
for (const exportName of ["createCertificateLoginChallenge", "signCertificateLoginChallenge", "verifyCertificateLoginProof", "createServerLoginProofEnvelope", "assertServerCredentialBoundary", "fingerprintDer", "runLocalCertificateLoginFixture"]) {
  require(typeof localCertificateAgent[exportName] !== "undefined", `local certificate agent export ${exportName}`, `local certificate agent missing export ${exportName}`);
}
const fixturePassword = "fixture-certificate-password!23";
const { privateKey: fixturePrivateKey, publicKey: fixturePublicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const fixtureSignPriKeyDer = fixturePrivateKey.export({
  type: "pkcs8",
  format: "der",
  cipher: "aes-256-cbc",
  passphrase: fixturePassword,
});
const fixtureSignCertDer = fixturePublicKey.export({ type: "spki", format: "der" });
const loginChallenge = localCertificateAgent.createCertificateLoginChallenge({
  institutionId: "4insure",
  workflowId: "certificate_login.fixture",
  sessionId: "fixture-session-001",
  nonce: "fixture-nonce-001",
  requestedScopes: ["business_social_insurance.read"],
  expiresAt: "2099-01-01T00:00:00.000Z",
  purpose: "fixture login proof only; no live filing",
});
require(loginChallenge.required_runtime === "LOCAL_AGENT_ONLY", "local certificate challenge requires local agent", "local certificate challenge must require local agent runtime");
require(loginChallenge.challenge_hash?.length === 64, "local certificate challenge has hash", "local certificate challenge must have hash");
const loginProof = localCertificateAgent.signCertificateLoginChallenge({
  runtimeZone: "local_agent",
  challenge: loginChallenge,
  signPriKeyDer: fixtureSignPriKeyDer,
  signCertDer: fixtureSignCertDer,
  certificatePassword: fixturePassword,
  approvedAt: "fixture-time",
});
require(loginProof.proof_type === "certificate_login_signed_challenge", "local certificate proof type", "local certificate proof type mismatch");
require(loginProof.signature_format === "LOCAL_AGENT_RSA_SHA256_POC", "local certificate proof declares POC signature format", "local certificate proof must declare fixture POC format");
require(loginProof.key_material_location === "LOCAL_AGENT_ONLY", "local certificate key material stays local", "local certificate proof must attest local-only key material");
require(loginProof.attestation.private_key_exported === false, "local certificate proof exports no private key", "local certificate proof must not export private key");
require(loginProof.attestation.certificate_password_returned === false, "local certificate proof returns no password", "local certificate proof must not return password");
require(loginProof.certificate_fingerprint_sha256 === localCertificateAgent.fingerprintDer(fixtureSignCertDer), "local certificate proof fingerprints cert DER", "local certificate proof must fingerprint signCert.der bytes");
require(localCertificateAgent.verifyCertificateLoginProof({ challenge: loginChallenge, proof: loginProof, signCertDer: fixtureSignCertDer }) === true, "local certificate proof verifies", "local certificate proof should verify");
let wrongPasswordBlocked = false;
try {
  localCertificateAgent.signCertificateLoginChallenge({
    runtimeZone: "local_agent",
    challenge: loginChallenge,
    signPriKeyDer: fixtureSignPriKeyDer,
    signCertDer: fixtureSignCertDer,
    certificatePassword: "wrong-password",
  });
} catch (error) {
  wrongPasswordBlocked = /failed to decrypt or use signPri.key/.test(error.message);
}
require(wrongPasswordBlocked, "local certificate rejects wrong password", "local certificate signer must reject wrong certificate password");
let serverSigningBlocked = false;
try {
  localCertificateAgent.signCertificateLoginChallenge({
    runtimeZone: "server",
    challenge: loginChallenge,
    signPriKeyDer: fixtureSignPriKeyDer,
    signCertDer: fixtureSignCertDer,
    certificatePassword: fixturePassword,
  });
} catch (error) {
  serverSigningBlocked = /local_agent runtime/.test(error.message);
}
require(serverSigningBlocked, "local certificate signing rejects server runtime", "local certificate signing must reject server runtime");
let expiredChallengeBlocked = false;
try {
  const expiredChallenge = localCertificateAgent.createCertificateLoginChallenge({
    institutionId: "4insure",
    workflowId: "certificate_login.fixture",
    sessionId: "fixture-session-001",
    nonce: "fixture-nonce-002",
    expiresAt: "2000-01-01T00:00:00.000Z",
    purpose: "expired fixture",
  });
  localCertificateAgent.signCertificateLoginChallenge({
    runtimeZone: "local_agent",
    challenge: expiredChallenge,
    signPriKeyDer: fixtureSignPriKeyDer,
    signCertDer: fixtureSignCertDer,
    certificatePassword: fixturePassword,
  });
} catch (error) {
  expiredChallengeBlocked = /challenge is expired/.test(error.message);
}
require(expiredChallengeBlocked, "local certificate rejects expired challenge", "local certificate signing must reject expired challenge");
let serverCredentialBoundaryBlocked = false;
try {
  localCertificateAgent.assertServerCredentialBoundary({
    signPriKeyDer: fixtureSignPriKeyDer.toString("base64"),
    signCertDer: fixtureSignCertDer.toString("base64"),
    certificatePassword: fixturePassword,
  });
} catch (error) {
  serverCredentialBoundaryBlocked = /forbidden server credential field/.test(error.message);
}
require(serverCredentialBoundaryBlocked, "server boundary rejects certificate material", "server boundary must reject certificate material");
const serverEnvelope = localCertificateAgent.createServerLoginProofEnvelope({
  connectorId: "local_certificate_login_fixture",
  challenge: loginChallenge,
  proof: loginProof,
});
require(serverEnvelope.accepted_boundary === "SIGNED_PROOF_ONLY", "server envelope accepts signed proof only", "server envelope must be signed-proof only");
require(serverEnvelope.proof.signature === loginProof.signature, "server envelope carries signature", "server envelope should carry proof signature");
localCertificateAgent.assertServerCredentialBoundary(serverEnvelope);
const localCertificateSerialized = JSON.stringify({ proof: loginProof, envelope: serverEnvelope, fixture: json(localCertificateAgentFixturePath) });
for (const forbidden of [fixturePassword, fixtureSignPriKeyDer.toString("base64"), "certificatePassword", "signPriKeyDer", "session_cookie", "localStorage:"]) {
  require(!localCertificateSerialized.includes(forbidden), `local certificate output avoids ${forbidden.slice(0, 24)}`, `local certificate output leaked ${forbidden.slice(0, 24)}`);
}
const localCertificateFixtureRun = localCertificateAgent.runLocalCertificateLoginFixture();
require(localCertificateFixtureRun.proof.proof_type === "certificate_login_signed_challenge", "local certificate fixture returns signed proof", "local certificate fixture should return signed proof");
require(localCertificateFixtureRun.server_envelope.accepted_boundary === "SIGNED_PROOF_ONLY", "local certificate fixture returns proof envelope", "local certificate fixture should return proof envelope");
requireIncludes(localCertificateAgentSpecPath, "signPri.key, signCert.der, and certificate password never leave the local agent", "local certificate spec forbids key/password crossing");
requireIncludes(localCertificateAgentSpecPath, "This module is not a production 공동인증서 CMS/VID implementation", "local certificate spec records POC boundary");

const nhisLossReport = await import("./korean-connectivity/nhis-edi-loss-report.mjs");
for (const exportName of ["NHIS_LOSS_REPORT_SOURCE_URLS", "NHIS_LOSS_REPORT_COLUMNS", "validateLossReportRows", "renderLossReportCsv", "assertNoLiveSubmission", "generateNhisLossReportFixture", "loadFixtureRows"]) {
  require(typeof nhisLossReport[exportName] !== "undefined", `NHIS loss report export ${exportName}`, `NHIS loss report missing export ${exportName}`);
}
const nhisRows = await nhisLossReport.loadFixtureRows(nhisLossReportFixturePath);
require(nhisRows.length === 2, "NHIS loss report fixture has two rows", "NHIS loss report fixture should have two rows");
const nhisCsv = nhisLossReport.renderLossReportCsv(nhisRows);
for (const header of ["사업장관리번호", "성명", "주민등록번호(대체토큰)", "상실일", "상실부호"]) {
  require(nhisCsv.includes(header), `NHIS loss report CSV header ${header}`, `NHIS CSV missing header ${header}`);
}
require(!/\b\d{6}-?[1-4]\d{6}\b/.test(nhisCsv), "NHIS loss report CSV has no real RRN pattern", "NHIS CSV must not expose real RRN pattern");
const nhisResult = await nhisLossReport.generateNhisLossReportFixture({ rows: nhisRows, outputPath: ".tmp/nhis-edi-loss-report.check.csv" });
require(nhisResult.execution_mode === "fixture_only", "NHIS loss report execution is fixture-only", "NHIS loss report must be fixture-only");
require(nhisResult.side_effect_class === "generated_file", "NHIS loss report side effect is generated_file", "NHIS loss report side effect must be generated_file");
require(nhisResult.row_count === 2, "NHIS loss report generated two rows", "NHIS loss report should generate two rows");
require(nhisResult.evidence_record.source_urls.includes("https://edi.nhis.or.kr/webedi/file_sy/all_sangsil.html"), "NHIS evidence includes EDI source", "NHIS evidence must include EDI source");
let liveSubmissionBlocked = false;
try {
  nhisLossReport.assertNoLiveSubmission();
} catch (error) {
  liveSubmissionBlocked = /live submission is prohibited/.test(error.message);
}
require(liveSubmissionBlocked, "NHIS live submission is blocked", "NHIS live submission must be blocked");
requireIncludes(nhisLossReportSpecPath, "does not submit a filing", "NHIS runbook records no live filing");
requireIncludes(nhisLossReportSpecPath, "human reviewer confirms", "NHIS runbook requires human review");

const forbiddenTokens = ["real_customer_password", "real_certificate_password", "production_session_cookie", "live_hometax_login"];
const combined = `${read(adr)}\n${read(spec)}\n${JSON.stringify(fixture)}`;
for (const token of forbiddenTokens) {
  require(!combined.includes(token), `no forbidden fixture token ${token}`, `forbidden fixture token present: ${token}`);
}

if (failures.length) {
  console.error("Korean institutional connectivity gate failed:\n" + failures.map((failure) => `- ${failure}`).join("\n"));
  process.exit(1);
}

console.log(`Korean institutional connectivity gate passed (${passes.length} checks).`);
for (const pass of passes) {
  console.log(`- ${pass}`);
}
