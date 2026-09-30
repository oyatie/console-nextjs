import { createEvidenceRecord, assertEvidenceSafe } from "./evidence-ledger.mjs";
import { createFixtureConnector, executeFixtureWorkflow } from "./adapter-sdk.mjs";

export const OPEN_BANKING_SOURCE_URLS = [
  "https://openapi.kftc.or.kr/service/openBanking",
  "https://developers.kftc.or.kr/dev/openapi/open-banking/transaction",
];

export function createOpenBankingFixtureConnector() {
  return createFixtureConnector({
    connector_id: "kftc_open_banking",
    parser_version: "kftc-open-banking-fixture-v1",
    side_effect_class: "read_only",
    source_urls: OPEN_BANKING_SOURCE_URLS,
    auth_shape: {
      mode: "official_oauth",
      token_storage: "fixture_only_no_real_token",
      account_registration: "fixture_fintech_use_num_only",
    },
    workflows: [
      {
        workflow_id: "banking.accounts.list",
        side_effect_class: "read_only",
        fixture_path: "docs/benchmarks/fixtures/korean-connectivity/kftc-open-banking.fixture.json",
      },
      {
        workflow_id: "banking.transactions.list",
        side_effect_class: "read_only",
        fixture_path: "docs/benchmarks/fixtures/korean-connectivity/kftc-open-banking.fixture.json",
      },
    ],
  });
}

export async function runOpenBankingFixture(workflowId, payload = {}) {
  const connector = createOpenBankingFixtureConnector();
  const run = await executeFixtureWorkflow(connector, workflowId, payload);
  const output = workflowId === "banking.transactions.list"
    ? { transactions: run.output.transactions }
    : { accounts: run.output.accounts };
  const evidence = createEvidenceRecord({
    connectorId: connector.connector_id,
    workflowId,
    intentHash: run.intent_hash,
    parserVersion: connector.parser_version,
    sourceUrls: OPEN_BANKING_SOURCE_URLS,
    transcript: `KFTC Open Banking fixture workflow ${workflowId}; no access_token; no bank password; fintech_use_num=${payload.fintech_use_num ?? "fixture-use-num"}`,
    output,
    observedAt: "fixture-time",
  });
  assertEvidenceSafe(evidence);
  return {
    ...run,
    output,
    evidence_record: evidence,
  };
}
