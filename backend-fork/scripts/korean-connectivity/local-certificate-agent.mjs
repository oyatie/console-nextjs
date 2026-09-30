#!/usr/bin/env node
import {
  X509Certificate,
  createHash,
  createPrivateKey,
  createPublicKey,
  createSign,
  createVerify,
  generateKeyPairSync,
} from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createEvidenceRecord, assertEvidenceSafe } from "./evidence-ledger.mjs";
import { sha256, stableJson } from "./adapter-sdk.mjs";

const SIGNATURE_FORMAT = "LOCAL_AGENT_RSA_SHA256_POC";
const PROOF_TYPE = "certificate_login_signed_challenge";
const LOCAL_RUNTIME = "local_agent";
const SERVER_FORBIDDEN_KEYS = new Set([
  "pfx",
  "p12",
  "privateKey",
  "private_key",
  "signPri.key",
  "signPriKey",
  "signPriKeyDer",
  "signCert.der",
  "signCertDer",
  "certificatePassword",
  "certificate_password",
  "certPassword",
  "password",
  "bank_password",
  "otp",
  "security_card",
  "securityCard",
  "session_cookie",
  "cookie",
  "localStorage",
  "sessionStorage",
]);
const SERVER_FORBIDDEN_PATTERNS = [
  /-----BEGIN [A-Z ]*PRIVATE KEY-----/i,
  /\bsignPri\.key\b/i,
  /\bsignCert\.der\b/i,
  /\.p(?:fx|12)\b/i,
  /\bcertificatePassword\b/i,
  /\bsession_cookie\b/i,
  /\blocalStorage\s*[:=]/i,
  /\bsessionStorage\s*[:=]/i,
];

function asBuffer(value, label) {
  if (Buffer.isBuffer(value)) return Buffer.from(value);
  if (value instanceof Uint8Array) return Buffer.from(value);
  if (typeof value === "string") return Buffer.from(value, "base64");
  throw new Error(`${label} must be DER bytes or base64 DER`);
}

function assertNonEmptyString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label} is required`);
  }
  return value.trim();
}

function parseExpiresAt(expiresAt) {
  const value = assertNonEmptyString(expiresAt, "expiresAt");
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) {
    throw new Error("expiresAt must be an ISO date-time");
  }
  return new Date(timestamp).toISOString();
}

function assertChallengeFresh(challenge, now = new Date()) {
  const expiresAt = Date.parse(challenge?.expires_at);
  if (!Number.isFinite(expiresAt)) {
    throw new Error("challenge expires_at is invalid");
  }
  if (expiresAt <= now.getTime()) {
    throw new Error("challenge is expired");
  }
}

export function fingerprintDer(derBytes) {
  return createHash("sha256").update(asBuffer(derBytes, "DER bytes")).digest("hex");
}

export function createCertificateLoginChallenge({
  institutionId,
  workflowId,
  sessionId,
  nonce,
  requestedScopes = [],
  expiresAt,
  purpose,
}) {
  const normalizedScopes = [...new Set((requestedScopes ?? []).map((scope) => assertNonEmptyString(scope, "requestedScopes[]")))].sort();
  const body = {
    institution_id: assertNonEmptyString(institutionId, "institutionId"),
    workflow_id: assertNonEmptyString(workflowId, "workflowId"),
    session_id: assertNonEmptyString(sessionId, "sessionId"),
    nonce: assertNonEmptyString(nonce, "nonce"),
    requested_scopes: normalizedScopes,
    expires_at: parseExpiresAt(expiresAt),
    purpose: assertNonEmptyString(purpose, "purpose"),
    required_runtime: "LOCAL_AGENT_ONLY",
    key_material_policy: "signPri.key, signCert.der, and certificate password never leave the local agent",
    proof_boundary: "SIGNED_PROOF_ONLY",
  };
  const canonicalPayload = stableJson(body);
  const challengeHash = sha256(canonicalPayload);
  return {
    challenge_id: `kic-cert-${challengeHash.slice(0, 32)}`,
    ...body,
    canonical_payload: canonicalPayload,
    challenge_hash: challengeHash,
  };
}

function privateKeyFromSignPriDer(signPriKeyDer, certificatePassword) {
  const keyBytes = asBuffer(signPriKeyDer, "signPri.key");
  const passphrase = assertNonEmptyString(certificatePassword, "certificatePassword");
  try {
    return createPrivateKey({ key: keyBytes, format: "der", type: "pkcs8", passphrase });
  } catch (error) {
    throw new Error(`failed to decrypt or use signPri.key: ${error.message}`);
  } finally {
    keyBytes.fill(0);
  }
}

function publicKeyFromSignCertDer(signCertDer) {
  const certBytes = asBuffer(signCertDer, "signCert.der");
  try {
    return new X509Certificate(certBytes).publicKey;
  } catch {
    try {
      return createPublicKey({ key: certBytes, format: "der", type: "spki" });
    } catch (error) {
      throw new Error(`failed to use signCert.der as X.509 certificate or SPKI public key: ${error.message}`);
    }
  }
}

function assertAllowedLocalCertificateOutput(output) {
  const serialized = JSON.stringify(output);
  for (const key of SERVER_FORBIDDEN_KEYS) {
    if (Object.hasOwn(output ?? {}, key) || serialized.includes(`"${key}"`)) {
      throw new Error(`forbidden local certificate output key: ${key}`);
    }
  }
  for (const pattern of SERVER_FORBIDDEN_PATTERNS) {
    if (pattern.test(serialized)) {
      throw new Error(`forbidden local certificate output pattern: ${pattern}`);
    }
  }
  return output;
}

export function signCertificateLoginChallenge({
  runtimeZone,
  challenge,
  signPriKeyDer,
  signCertDer,
  certificatePassword,
  approvedAt = new Date().toISOString(),
}) {
  if (runtimeZone !== LOCAL_RUNTIME) {
    throw new Error("certificate signing requires local_agent runtime");
  }
  if (!challenge?.canonical_payload || !challenge?.challenge_hash) {
    throw new Error("challenge with canonical_payload and challenge_hash is required");
  }
  if (sha256(challenge.canonical_payload) !== challenge.challenge_hash) {
    throw new Error("challenge hash does not match canonical payload");
  }
  assertChallengeFresh(challenge);
  const privateKey = privateKeyFromSignPriDer(signPriKeyDer, certificatePassword);
  const sign = createSign("RSA-SHA256");
  sign.update(challenge.canonical_payload);
  sign.end();
  const signature = sign.sign(privateKey).toString("base64");
  const proof = {
    proof_type: PROOF_TYPE,
    signature_format: SIGNATURE_FORMAT,
    challenge_id: challenge.challenge_id,
    challenge_hash: challenge.challenge_hash,
    signature,
    certificate_fingerprint_sha256: fingerprintDer(signCertDer),
    signed_at: approvedAt,
    key_material_location: "LOCAL_AGENT_ONLY",
    allowed_crossing: ["challenge_hash", "signature", "certificate_fingerprint_sha256", "attestation"],
    attestation: {
      local_agent_version: "local-certificate-agent-poc-v1",
      customer_controlled_runtime: true,
      user_approved: true,
      private_key_exported: false,
      certificate_password_returned: false,
      raw_certificate_returned: false,
      production_login_performed: false,
      live_submission_performed: false,
    },
  };
  if (!verifyCertificateLoginProof({ challenge, proof, signCertDer })) {
    throw new Error("signCert.der does not verify the generated challenge signature");
  }
  return assertAllowedLocalCertificateOutput(proof);
}

export function verifyCertificateLoginProof({ challenge, proof, signCertDer }) {
  if (proof?.proof_type !== PROOF_TYPE || proof?.signature_format !== SIGNATURE_FORMAT) return false;
  if (proof.challenge_hash !== challenge?.challenge_hash || proof.challenge_id !== challenge?.challenge_id) return false;
  if (proof.certificate_fingerprint_sha256 !== fingerprintDer(signCertDer)) return false;
  const verify = createVerify("RSA-SHA256");
  verify.update(challenge.canonical_payload);
  verify.end();
  return verify.verify(publicKeyFromSignCertDer(signCertDer), Buffer.from(proof.signature, "base64"));
}

function scanForbiddenServerKeys(value, path = "payload") {
  if (!value || typeof value !== "object") return null;
  for (const [key, child] of Object.entries(value)) {
    if (SERVER_FORBIDDEN_KEYS.has(key)) return `${path}.${key}`;
    const nested = scanForbiddenServerKeys(child, `${path}.${key}`);
    if (nested) return nested;
  }
  return null;
}

export function assertServerCredentialBoundary(envelope) {
  const forbiddenKeyPath = scanForbiddenServerKeys(envelope);
  if (forbiddenKeyPath) {
    throw new Error(`forbidden server credential field: ${forbiddenKeyPath}`);
  }
  const serialized = JSON.stringify(envelope ?? {});
  const pattern = SERVER_FORBIDDEN_PATTERNS.find((candidate) => candidate.test(serialized));
  if (pattern) {
    throw new Error(`forbidden server credential pattern: ${pattern}`);
  }
  return true;
}

export function createServerLoginProofEnvelope({ connectorId, challenge, proof }) {
  if (proof?.challenge_hash !== challenge?.challenge_hash) {
    throw new Error("proof does not match challenge");
  }
  const envelope = {
    connector_id: assertNonEmptyString(connectorId, "connectorId"),
    workflow_id: challenge.workflow_id,
    accepted_boundary: "SIGNED_PROOF_ONLY",
    challenge_id: challenge.challenge_id,
    challenge_hash: challenge.challenge_hash,
    proof: {
      proof_type: proof.proof_type,
      signature_format: proof.signature_format,
      signature: proof.signature,
      certificate_fingerprint_sha256: proof.certificate_fingerprint_sha256,
      signed_at: proof.signed_at,
      attestation: proof.attestation,
    },
    server_handling: {
      stores_key_material: false,
      stores_certificate_password: false,
      stores_browser_session_material: false,
      may_exchange_for_fixture_session: true,
      may_perform_live_submission: false,
    },
  };
  assertServerCredentialBoundary(envelope);
  return envelope;
}

export function runLocalCertificateLoginFixture({
  institutionId = "4insure",
  workflowId = "certificate_login.fixture",
  sessionId = "fixture-session-001",
  nonce = "fixture-nonce-001",
  requestedScopes = ["business_social_insurance.read"],
  expiresAt = "2099-01-01T00:00:00.000Z",
  purpose = "fixture local certificate login proof only; no live filing",
  approvedAt = "fixture-time",
} = {}) {
  const fixturePassword = "fixture-certificate-password-not-exported";
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const signPriKeyDer = privateKey.export({
    type: "pkcs8",
    format: "der",
    cipher: "aes-256-cbc",
    passphrase: fixturePassword,
  });
  const signCertDer = publicKey.export({ type: "spki", format: "der" });
  const challenge = createCertificateLoginChallenge({ institutionId, workflowId, sessionId, nonce, requestedScopes, expiresAt, purpose });
  const proof = signCertificateLoginChallenge({
    runtimeZone: LOCAL_RUNTIME,
    challenge,
    signPriKeyDer,
    signCertDer,
    certificatePassword: fixturePassword,
    approvedAt,
  });
  const serverEnvelope = createServerLoginProofEnvelope({
    connectorId: "local_certificate_login_fixture",
    challenge,
    proof,
  });
  const evidenceRecord = createEvidenceRecord({
    connectorId: "local_certificate_login_fixture",
    workflowId,
    intentHash: challenge.challenge_hash,
    parserVersion: "local-certificate-agent-poc-v1",
    sourceUrls: [
      "https://www.law.go.kr/LSW//lsSideInfoP.do?docCls=jo&joBrNo=00&joNo=0003&lsiSeq=236201&urlMode=lsScJoRltInfoR",
    ],
    transcript: `${institutionId}/${workflowId} fixture challenge signed in local agent only; no live login; no live filing`,
    output: serverEnvelope,
    observedAt: "fixture-time",
  });
  assertEvidenceSafe(evidenceRecord);
  return {
    execution_mode: "fixture_only",
    runtime_zone: "local_agent",
    challenge,
    proof,
    server_envelope: serverEnvelope,
    evidence_record: evidenceRecord,
  };
}

function parseArgs(argv) {
  const args = {
    outPath: ".tmp/local-certificate-login.fixture.json",
    allowLocalKeyFiles: false,
    certPasswordEnv: "KIC_CERT_PASSWORD",
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--out") args.outPath = argv[++i];
    else if (arg === "--allow-local-key-files") args.allowLocalKeyFiles = true;
    else if (arg === "--sign-pri-key") args.signPriKeyPath = argv[++i];
    else if (arg === "--sign-cert-der") args.signCertDerPath = argv[++i];
    else if (arg === "--cert-password-env") args.certPasswordEnv = argv[++i];
    else if (arg === "--institution-id") args.institutionId = argv[++i];
    else if (arg === "--workflow-id") args.workflowId = argv[++i];
    else if (arg === "--session-id") args.sessionId = argv[++i];
    else if (arg === "--nonce") args.nonce = argv[++i];
    else if (arg === "--expires-at") args.expiresAt = argv[++i];
    else if (arg === "--purpose") args.purpose = argv[++i];
    else if (arg === "--scope") {
      args.requestedScopes ??= [];
      args.requestedScopes.push(argv[++i]);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return args;
}

async function runCli(argv) {
  const args = parseArgs(argv);
  let result;
  if (args.allowLocalKeyFiles) {
    if (!args.signPriKeyPath || !args.signCertDerPath) {
      throw new Error("--allow-local-key-files requires --sign-pri-key and --sign-cert-der");
    }
    const certificatePassword = process.env[args.certPasswordEnv];
    if (!certificatePassword) {
      throw new Error(`${args.certPasswordEnv} must contain the certificate password; do not pass it on the command line`);
    }
    const challenge = createCertificateLoginChallenge({
      institutionId: args.institutionId ?? "fixture_institution",
      workflowId: args.workflowId ?? "certificate_login.fixture",
      sessionId: args.sessionId ?? `local-${Date.now()}`,
      nonce: args.nonce ?? `nonce-${Date.now()}`,
      requestedScopes: args.requestedScopes ?? ["fixture.read"],
      expiresAt: args.expiresAt ?? new Date(Date.now() + 5 * 60 * 1000).toISOString(),
      purpose: args.purpose ?? "local certificate login proof fixture; no live submission",
    });
    const signPriKeyDer = await readFile(args.signPriKeyPath);
    const signCertDer = await readFile(args.signCertDerPath);
    const proof = signCertificateLoginChallenge({
      runtimeZone: LOCAL_RUNTIME,
      challenge,
      signPriKeyDer,
      signCertDer,
      certificatePassword,
    });
    result = {
      execution_mode: "local_only_no_network",
      runtime_zone: "local_agent",
      challenge,
      proof,
      server_envelope: createServerLoginProofEnvelope({ connectorId: "local_certificate_login_manual", challenge, proof }),
      warning: "This is a local proof generator only. It does not perform live institution login, CMS/VID production signing, or filing.",
    };
  } else {
    result = runLocalCertificateLoginFixture(args);
  }
  await mkdir(dirname(args.outPath), { recursive: true });
  await writeFile(args.outPath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
  console.log(JSON.stringify({ ok: true, out: args.outPath, execution_mode: result.execution_mode, accepted_boundary: result.server_envelope.accepted_boundary }, null, 2));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  runCli(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
