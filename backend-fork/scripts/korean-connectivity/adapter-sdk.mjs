import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const LIVE_URL_PATTERN = /^https?:\/\//i;

export class ConnectivityPolicyError extends Error {
  constructor(message, details = {}) {
    super(message);
    this.name = "ConnectivityPolicyError";
    this.details = details;
  }
}

export function stableJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function sha256(value) {
  return createHash("sha256").update(typeof value === "string" ? value : stableJson(value)).digest("hex");
}

export function assertNoLiveUrl(url, context = "fixture connector") {
  if (typeof url === "string" && LIVE_URL_PATTERN.test(url)) {
    throw new ConnectivityPolicyError(`${context} attempted live network access`, { url });
  }
}

export function assertFixtureConnector(connector) {
  if (!connector || typeof connector !== "object") {
    throw new ConnectivityPolicyError("connector definition is required");
  }
  if (connector.execution_mode !== "fixture_only") {
    throw new ConnectivityPolicyError("connector execution_mode must be fixture_only", {
      connector_id: connector.connector_id,
      execution_mode: connector.execution_mode,
    });
  }
  if (!connector.connector_id || !Array.isArray(connector.workflows) || connector.workflows.length === 0) {
    throw new ConnectivityPolicyError("connector must define connector_id and workflows");
  }
  if (connector.live_endpoint) {
    assertNoLiveUrl(connector.live_endpoint, connector.connector_id);
  }
  return connector;
}

export function createFixtureConnector(definition) {
  const connector = {
    execution_mode: "fixture_only",
    parser_version: definition.parser_version ?? "fixture-parser-v1",
    ...definition,
  };
  return assertFixtureConnector(connector);
}

export function prepareIntent({ connector_id, workflow_id, payload, side_effect_class = "read_only" }) {
  if (!connector_id || !workflow_id) {
    throw new ConnectivityPolicyError("connector_id and workflow_id are required to prepare intent");
  }
  const normalized = {
    connector_id,
    workflow_id,
    side_effect_class,
    payload: payload ?? {},
  };
  return {
    ...normalized,
    intent_hash: sha256(normalized),
  };
}

export async function loadFixture(path) {
  assertNoLiveUrl(path, "fixture path");
  const abs = resolve(path);
  return JSON.parse(await readFile(abs, "utf8"));
}

export async function executeFixtureWorkflow(connector, workflow_id, payload = {}) {
  assertFixtureConnector(connector);
  const workflow = connector.workflows.find((candidate) => candidate.workflow_id === workflow_id);
  if (!workflow) {
    throw new ConnectivityPolicyError("unknown fixture workflow", {
      connector_id: connector.connector_id,
      workflow_id,
    });
  }
  if (workflow.fixture_path) {
    assertNoLiveUrl(workflow.fixture_path, `${connector.connector_id}.${workflow_id}`);
  }
  const intent = prepareIntent({
    connector_id: connector.connector_id,
    workflow_id,
    payload,
    side_effect_class: workflow.side_effect_class ?? connector.side_effect_class ?? "read_only",
  });
  const fixture = await loadFixture(workflow.fixture_path ?? connector.fixture_path);
  return {
    connector_id: connector.connector_id,
    workflow_id,
    execution_mode: "fixture_only",
    parser_version: connector.parser_version,
    intent_hash: intent.intent_hash,
    evidence: {
      source_urls: connector.source_urls ?? [],
      fixture_path: workflow.fixture_path ?? connector.fixture_path,
      deterministic_fixture_hash: sha256(fixture),
    },
    output: fixture.output ?? fixture,
  };
}

export async function withNoLiveNetworkGuard(callback) {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input) => {
    const url = typeof input === "string" ? input : input?.url;
    assertNoLiveUrl(url, "global fetch");
    throw new ConnectivityPolicyError("fetch is disabled in fixture-only connectivity mode", { url });
  };
  try {
    return await callback();
  } finally {
    if (originalFetch === undefined) {
      delete globalThis.fetch;
    } else {
      globalThis.fetch = originalFetch;
    }
  }
}
