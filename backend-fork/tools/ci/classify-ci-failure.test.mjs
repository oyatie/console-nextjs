import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { classifyLogText } from "./classify-ci-failure.mjs";

describe("classify-ci-failure", () => {
  it("classifies action-download Service Unavailable as infra flake", () => {
    const log = `
Prepare all required actions
Getting action download info
Failed to resolve action download info. Error: Service Unavailable
##[error]Service Unavailable
##[error]Failed to resolve action download info.
`;
    const r = classifyLogText(log);
    assert.equal(r.class_id, "ops.gha-infra-flake");
    assert.equal(r.product, false);
  });

  it("classifies rust compile errors as product", () => {
    const log = `
##[group]Run cargo test
Compiling console-policy-domain
error[E0308]: mismatched types
test result: FAILED. 1 passed; 1 failed
`;
    const r = classifyLogText(log);
    assert.equal(r.class_id, "product.ci-failure");
    assert.equal(r.product, true);
  });
});
