import { createHmac } from "node:crypto";
import { createEvidenceRecord, assertEvidenceSafe } from "./evidence-ledger.mjs";
import { prepareIntent, stableJson } from "./adapter-sdk.mjs";

const FIXTURE_HMAC_KEY = "fixture-local-agent-nonsecret-key-not-a-certificate";
const FORBIDDEN_OUTPUT_KEYS = new Set([
  "pfx",
  "privateKey",
  "private_key",
  "signPri.key",
  "signCert.der",
  "certPassword",
  "certificate_password",
  "password",
  "otp",
  "security_card",
  "session_cookie",
  "cookie",
  "localStorage",
  "sessionStorage",
]);

export function createLocalAgentIntent({ institutionId, workflowId, consentId, sideEffectClass, payload }) {
  if (!institutionId || !workflowId || !consentId) {
    throw new Error("institutionId, workflowId, and consentId are required");
  }
  return prepareIntent({
    connector_id: "local_agent_cert_session_simulator",
    workflow_id: workflowId,
    side_effect_class: sideEffectClass ?? "read_only",
    payload: {
      institution_id: institutionId,
      consent_id: consentId,
      payload: payload ?? {},
    },
  });
}

export function simulateLocalAgentSignature(intent, { agentVersion = "local-agent-simulator-v1", approvedAt = "fixture-time" } = {}) {
  if (!intent?.intent_hash) {
    throw new Error("intent_hash is required");
  }
  const signature = createHmac("sha256", FIXTURE_HMAC_KEY)
    .update(intent.intent_hash)
    .update(stableJson({ agentVersion, approvedAt }))
    .digest("hex");
  return assertAllowedLocalAgentOutput({
    proof_type: "fixture_signed_challenge",
    signature_algorithm: "fixture_hmac_sha256_not_a_certificate",
    signed_challenge: signature,
    intent_hash: intent.intent_hash,
    attestation: {
      local_agent_version: agentVersion,
      approved_at: approvedAt,
      customer_controlled_runtime: true,
      real_certificate_used: false,
      private_key_exported: false,
      credential_material_returned: false,
    },
    allowed_artifacts: ["signed_challenge", "intent_hash", "attestation_metadata"],
  });
}

export function assertAllowedLocalAgentOutput(output) {
  const serialized = JSON.stringify(output);
  for (const key of FORBIDDEN_OUTPUT_KEYS) {
    if (Object.hasOwn(output ?? {}, key) || serialized.includes(`"${key}"`)) {
      throw new Error(`forbidden local-agent output key: ${key}`);
    }
  }
  for (const pattern of [/signPri\.key/i, /signCert\.der/i, /\.pfx\b/i, /-----BEGIN [A-Z ]*PRIVATE KEY-----/i, /Cookie\s*:/i]) {
    if (pattern.test(serialized)) {
      throw new Error(`forbidden local-agent output pattern: ${pattern}`);
    }
  }
  return output;
}

export function runLocalAgentFixture({ institutionId = "nhis_edi_loss_report", workflowId = "local_agent.signed_challenge.fixture", consentId = "fixture-consent-001", payload = {} } = {}) {
  const intent = createLocalAgentIntent({
    institutionId,
    workflowId,
    consentId,
    sideEffectClass: "read_only",
    payload,
  });
  const proof = simulateLocalAgentSignature(intent);
  const evidence = createEvidenceRecord({
    connectorId: "local_agent_cert_session_simulator",
    workflowId,
    intentHash: intent.intent_hash,
    parserVersion: "local-agent-simulator-v1",
    sourceUrls: [
      "https://www.law.go.kr/lsInfoP.do?lsiSeq=102472",
      "https://www.law.go.kr/LSW//admRulInfoP.do?admRulSeq=2100000265956&chrClsCd=010201",
    ],
    transcript: `fixture local agent displayed ${institutionId}/${workflowId} and returned signed challenge only`,
    output: proof,
    observedAt: "fixture-time",
  });
  assertEvidenceSafe(evidence);
  return {
    intent,
    proof,
    evidence_record: evidence,
  };
}
