import { appendFile, mkdir } from "node:fs/promises";
import { dirname } from "node:path";
import { sha256, stableJson } from "./adapter-sdk.mjs";

const REDACTION_RULES = [
  { id: "rrn", label: "Korean resident registration number", pattern: /\b\d{6}-?[1-4]\d{6}\b/g, replacement: "[REDACTED_RRN]" },
  { id: "account_number", label: "Account-like number", pattern: /\b\d{2,6}-\d{2,6}-\d{2,8}(?:-\d{1,4})?\b/g, replacement: "[REDACTED_ACCOUNT]" },
  { id: "long_numeric_identifier", label: "Long numeric identifier", pattern: /\b\d{12,16}\b/g, replacement: "[REDACTED_NUMERIC_ID]" },
  { id: "cookie_header", label: "Cookie header", pattern: /\b(?:set-cookie|cookie)\s*:[^;\n\r]+/gi, replacement: "[REDACTED_COOKIE_HEADER]" },
  { id: "password_field", label: "Password field", pattern: /("?(?:password|certPassword|certificate_password|bank_password)"?\s*[:=]\s*)"?[^";\n\r,}]+"?/gi, replacement: "$1[REDACTED_PASSWORD]" },
  { id: "otp_field", label: "OTP or security card field", pattern: /("?(?:otp|security_card|securityCard)"?\s*[:=]\s*)"?[^";\n\r,}]+"?/gi, replacement: "$1[REDACTED_OTP]" },
  { id: "private_key_block", label: "Private key block", pattern: /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g, replacement: "[REDACTED_PRIVATE_KEY_BLOCK]" },
  { id: "certificate_private_file", label: "Certificate private-key file", pattern: /\b(?:signPri\.key|signCert\.der|\.pfx|\.p12)\b/gi, replacement: "[REDACTED_CERT_FILE]" },
  { id: "session_storage", label: "Browser storage", pattern: /\b(?:localStorage|sessionStorage)\s*[:=][^\n\r]+/gi, replacement: "[REDACTED_BROWSER_STORAGE]" }
];

export function redactText(input) {
  let text = String(input ?? "");
  const findings = [];
  for (const rule of REDACTION_RULES) {
    let count = 0;
    text = text.replace(rule.pattern, (...args) => {
      count += 1;
      const maybeOffset = args.at(-2);
      if (typeof maybeOffset === "number") {
        findings.push({ rule_id: rule.id, label: rule.label, offset: maybeOffset });
      } else {
        findings.push({ rule_id: rule.id, label: rule.label });
      }
      return rule.replacement;
    });
    if (count === 0) continue;
  }
  return { text, findings };
}

export function redactJson(value) {
  const raw = typeof value === "string" ? value : stableJson(value);
  return redactText(raw);
}

export function createEvidenceRecord({
  connectorId,
  workflowId,
  intentHash,
  parserVersion,
  sourceUrls = [],
  transcript = "",
  output = {},
  retentionClass = "fixture",
  observedAt = "fixture-time",
}) {
  if (!connectorId || !workflowId || !intentHash || !parserVersion) {
    throw new Error("connectorId, workflowId, intentHash, and parserVersion are required");
  }
  const redactedTranscript = redactText(transcript);
  const redactedOutput = redactJson(output);
  const body = {
    connector_id: connectorId,
    workflow_id: workflowId,
    intent_hash: intentHash,
    parser_version: parserVersion,
    source_urls: [...sourceUrls].sort(),
    retention_class: retentionClass,
    observed_at: observedAt,
    redacted_transcript: redactedTranscript.text,
    redacted_output: redactedOutput.text,
    redaction_findings: [...redactedTranscript.findings, ...redactedOutput.findings].map(({ rule_id, label }) => ({ rule_id, label })),
  };
  return {
    evidence_id: sha256(body),
    ...body,
  };
}

export function assertEvidenceSafe(record) {
  const serialized = stableJson(record);
  const unsafePatterns = [
    /\b\d{6}-?[1-4]\d{6}\b/,
    /-----BEGIN [A-Z ]*PRIVATE KEY-----/,
    /\b(?:signPri\.key|signCert\.der|\.pfx|\.p12)\b/i,
    /\b(?:set-cookie|cookie)\s*:/i,
    /"?(?:password|certPassword|certificate_password|bank_password)"?\s*[:=]\s*"?(?!\[REDACTED_PASSWORD\])[^"\n\r,}]+"?/i,
    /"?(?:otp|security_card|securityCard)"?\s*[:=]\s*"?(?!\[REDACTED_OTP\])[^"\n\r,}]+"?/i,
  ];
  const hit = unsafePatterns.find((pattern) => pattern.test(serialized));
  if (hit) {
    throw new Error(`unsafe evidence record contains unredacted regulated data: ${hit}`);
  }
  return true;
}

export async function appendEvidenceRecord(ledgerPath, record) {
  assertEvidenceSafe(record);
  await mkdir(dirname(ledgerPath), { recursive: true });
  await appendFile(ledgerPath, `${stableJson(record)}\n`, "utf8");
  return record.evidence_id;
}
