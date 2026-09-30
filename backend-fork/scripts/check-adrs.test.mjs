import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import { evaluateAdrGovernance } from "./check-adrs.mjs";

const OMIT = Symbol("omit");

function yamlValue(value) {
  if (Array.isArray(value)) {
    return `[${value.join(", ")}]`;
  }
  return value;
}

function adr({
  id,
  status = "accepted",
  docStatus = "published",
  date = "2026-07-13",
  owner = "test-owner",
  related = [],
  relationships = {},
  body = "",
}) {
  const fields = {
    id,
    status,
    doc_status: docStatus,
    date,
    owner,
    ...relationships,
    related,
  };
  const lines = Object.entries(fields)
    .filter(([, value]) => value !== OMIT)
    .map(([key, value]) => `${key}: ${yamlValue(value)}`);

  const headingId = id === OMIT ? "ADR fixture" : id;
  return `---\n${lines.join("\n")}\n---\n\n# ${headingId}: Fixture decision\n\n${body}\n`;
}

function designNote({ id = "DN-0001", parent = "ADR-0001", date = "2026-07-13" } = {}) {
  return `---
id: ${id}
kind: design-note
parent_adr: ${parent}
authority: subordinate
activation: dark
date: ${date}
owner: test-owner
---

# ${id}: Fixture design note
`;
}

function createFixture({ adrs, notes = [], indexRows } = {}) {
  const root = mkdtempSync(join(tmpdir(), "check-adrs-"));
  const decisions = join(root, "docs", "decisions");
  mkdirSync(join(decisions, "notes"), { recursive: true });

  const records =
    adrs ??
    [
      {
        filename: "ADR-0001-first.md",
        content: adr({ id: "ADR-0001" }),
      },
    ];
  for (const { filename, content } of records) {
    writeFileSync(join(decisions, filename), content);
  }
  for (const { filename, content } of notes) {
    writeFileSync(join(decisions, "notes", filename), content);
  }

  const rows =
    indexRows ??
    records.map(({ filename, content }) => {
      const id = content.match(/^id:\s*(ADR-\d{4})$/m)?.[1] ?? "ADR-9999";
      const status = content.match(/^status:\s*([^\n]+)$/m)?.[1] ?? "accepted";
      return `| [${id}](${filename}) | ${status} | Fixture |`;
    });
  writeFileSync(
    join(decisions, "README.md"),
    `# Architecture decision records

| ID | Status | Decision and scope |
|---|---|---|
${rows.join("\n")}
| ADR-0013 | never issued | Plan-only APNs placeholder; reserved historical gap |
`,
  );

  return root;
}

function assertFailure(result, fragment) {
  assert.ok(
    result.failures.some((failure) => failure.includes(fragment)),
    `expected failure containing ${JSON.stringify(fragment)}; got ${JSON.stringify(result.failures)}`,
  );
}

describe("ADR governance gate", () => {
  it("uses node:path primitives instead of POSIX-only path splitting", () => {
    const source = readFileSync(new URL("./check-adrs.mjs", import.meta.url), "utf8");

    assert.doesNotMatch(source, /\.split\(["']\/["']\)/);
    assert.doesNotMatch(source, /startsWith\(`\$\{(?:root|decisionsDirectory)\}\//);
  });

  it("accepts a complete corpus and the reserved never-issued ADR-0013 index row", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-old.md",
          content: adr({
            id: "ADR-0001",
            status: "superseded",
            docStatus: "archived",
            related: ["ADR-0002"],
            relationships: { superseded_by: ["ADR-0002"] },
          }),
        },
        {
          filename: "ADR-0002-new.md",
          content: adr({
            id: "ADR-0002",
            related: ["ADR-0001"],
            relationships: { supersedes: ["ADR-0001"] },
          }),
        },
        {
          filename: "ADR-0003-proposal.md",
          content: adr({
            id: "ADR-0003",
            status: "proposed",
            docStatus: "review",
            relationships: { proposes_amendments_to: ["ADR-0002"] },
          }),
        },
      ],
      notes: [
        {
          filename: "DN-0001-adr-0003-detail.md",
          content: designNote({ parent: "ADR-0003" }),
        },
      ],
    });

    const result = evaluateAdrGovernance(root);

    assert.deepEqual(result.failures, []);
    assert.equal(result.adrCount, 3);
    assert.equal(result.noteCount, 1);
  });

  it("rejects duplicate ADR IDs", () => {
    const root = createFixture({
      adrs: [
        { filename: "ADR-0001-first.md", content: adr({ id: "ADR-0001" }) },
        { filename: "ADR-0001-second.md", content: adr({ id: "ADR-0001" }) },
      ],
      indexRows: ["| [ADR-0001](ADR-0001-first.md) | accepted | Fixture |"],
    });

    assertFailure(evaluateAdrGovernance(root), "duplicate ADR id ADR-0001");
  });

  for (const [field, override] of [
    ["id", { id: OMIT }],
    ["status", { status: OMIT }],
    ["doc_status", { docStatus: OMIT }],
    ["date", { date: OMIT }],
    ["owner", { owner: OMIT }],
    ["related", { related: OMIT }],
  ]) {
    it(`rejects missing required ${field} frontmatter`, () => {
      const root = createFixture({
        adrs: [
          {
            filename: "ADR-0001-invalid.md",
            content: adr({ id: "ADR-0001", ...override }),
          },
        ],
        indexRows: ["| [ADR-0001](ADR-0001-invalid.md) | accepted | Fixture |"],
      });

      assertFailure(evaluateAdrGovernance(root), `missing required frontmatter field ${field}`);
    });
  }

  it("rejects malformed and nonexistent local ADR relationship references", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-invalid.md",
          content: adr({
            id: "ADR-0001",
            related: ["DESIGN.md", "ADR-9999"],
          }),
        },
      ],
    });

    const result = evaluateAdrGovernance(root);
    assertFailure(result, "related reference DESIGN.md must be a local ADR id");
    assertFailure(result, "related reference ADR-9999 does not resolve");
  });

  it("rejects an impossible ADR calendar date", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-invalid-date.md",
          content: adr({ id: "ADR-0001", date: "2026-02-30" }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "date \"2026-02-30\" must be a real calendar date");
  });

  it("rejects an impossible design-note calendar date", () => {
    const root = createFixture({
      notes: [
        {
          filename: "DN-0001-invalid-date.md",
          content: designNote({ date: "2025-13-01" }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "date \"2025-13-01\" must be a real calendar date");
  });

  it("rejects one-sided amend and supersession relationships", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-old.md",
          content: adr({
            id: "ADR-0001",
            related: ["ADR-0002"],
            relationships: { amended_by: ["ADR-0002"] },
          }),
        },
        {
          filename: "ADR-0002-new.md",
          content: adr({ id: "ADR-0002", related: ["ADR-0001"] }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "ADR-0002 must declare amends: ADR-0001");
  });

  it("rejects a proposed ADR that claims amendment or supersession authority", () => {
    const root = createFixture({
      adrs: [
        { filename: "ADR-0001-accepted.md", content: adr({ id: "ADR-0001" }) },
        {
          filename: "ADR-0002-proposed.md",
          content: adr({
            id: "ADR-0002",
            status: "proposed",
            docStatus: "review",
            related: ["ADR-0001"],
            relationships: { supersedes: ["ADR-0001"] },
          }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "proposed ADR cannot declare supersedes");
  });

  it("rejects an accepted record that treats a proposed ADR as its superseder", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-old.md",
          content: adr({
            id: "ADR-0001",
            status: "superseded",
            docStatus: "archived",
            related: ["ADR-0002"],
            relationships: { superseded_by: ["ADR-0002"] },
          }),
        },
        {
          filename: "ADR-0002-proposed.md",
          content: adr({
            id: "ADR-0002",
            status: "proposed",
            docStatus: "review",
            related: ["ADR-0001"],
            relationships: { proposes_amendments_to: ["ADR-0001"] },
          }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "superseded_by target ADR-0002 must be accepted");
  });

  it("rejects an accepted ADR that amends a non-accepted target", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-proposed.md",
          content: adr({
            id: "ADR-0001",
            status: "proposed",
            docStatus: "review",
            related: ["ADR-0002"],
            relationships: { amended_by: ["ADR-0002"] },
          }),
        },
        {
          filename: "ADR-0002-accepted.md",
          content: adr({
            id: "ADR-0002",
            related: ["ADR-0001"],
            relationships: { amends: ["ADR-0001"] },
          }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "amends target ADR-0001 must be accepted, found proposed");
  });

  it("rejects an ADR that declares a superseder without becoming superseded", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-old.md",
          content: adr({
            id: "ADR-0001",
            related: ["ADR-0002"],
            relationships: { superseded_by: ["ADR-0002"] },
          }),
        },
        {
          filename: "ADR-0002-new.md",
          content: adr({
            id: "ADR-0002",
            related: ["ADR-0001"],
            relationships: { supersedes: ["ADR-0001"] },
          }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "ADR with superseded_by must have status superseded");
  });

  it("rejects index status that promotes a proposed ADR to accepted", () => {
    const root = createFixture({
      adrs: [
        {
          filename: "ADR-0001-proposed.md",
          content: adr({ id: "ADR-0001", status: "proposed", docStatus: "review" }),
        },
      ],
      indexRows: ["| [ADR-0001](ADR-0001-proposed.md) | accepted | Fixture |"],
    });

    assertFailure(evaluateAdrGovernance(root), "index status accepted does not match frontmatter status proposed");
  });

  it("rejects an index link that does not target the indexed ADR file", () => {
    const root = createFixture({
      indexRows: ["| [ADR-0001](ADR-0001-missing.md) | accepted | Fixture |"],
    });

    assertFailure(
      evaluateAdrGovernance(root),
      "index link ADR-0001-missing.md does not match ADR-0001-first.md",
    );
  });

  it("rejects Markdown files that evade the ADR or design-note filename contracts", () => {
    const root = createFixture();
    writeFileSync(join(root, "docs", "decisions", "ADR-0002.md"), adr({ id: "ADR-0002" }));
    writeFileSync(join(root, "docs", "decisions", "notes", "scheduling.md"), designNote());

    const result = evaluateAdrGovernance(root);
    assertFailure(result, "ADR-0002.md: unexpected Markdown file");
    assertFailure(result, "notes/scheduling.md: unexpected Markdown file");
  });

  it("rejects stale cross-repository references that still use ADR-0022 for portability", () => {
    const root = createFixture();
    mkdirSync(join(root, "deploy"), { recursive: true });
    writeFileSync(
      join(root, "deploy", "README.md"),
      "The ADR-0022 bare-metal portability and HA target remains DARK.\n",
    );

    assertFailure(
      evaluateAdrGovernance(root),
      "ADR-0022 is the local-identity decision, not the portability/HA decision",
    );
  });

  it("scans tracked IaC text for retired ADR identities", () => {
    const root = createFixture();
    mkdirSync(join(root, "deploy", "opentofu"), { recursive: true });
    writeFileSync(
      join(root, "deploy", "opentofu", "main.tf"),
      '# Governed by ADR-0022-bare-metal-portability-and-ha.\n',
    );

    assertFailure(
      evaluateAdrGovernance(root),
      "ADR-0022 is the local-identity decision, not the portability/HA decision",
    );
  });

  it("rejects a proposed ADR whose prose claims active supersession", () => {
    const root = createFixture({
      adrs: [
        { filename: "ADR-0001-accepted.md", content: adr({ id: "ADR-0001" }) },
        {
          filename: "ADR-0002-proposed.md",
          content: adr({
            id: "ADR-0002",
            status: "proposed",
            docStatus: "review",
            related: ["ADR-0001"],
            body: "This ADR supersedes ADR-0001.",
          }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "proposed ADR prose claims active amendment or supersession");
  });

  it("rejects a proposed ADR whose subject ID claims active supersession in prose", () => {
    const root = createFixture({
      adrs: [
        { filename: "ADR-0001-accepted.md", content: adr({ id: "ADR-0001" }) },
        {
          filename: "ADR-0002-proposed.md",
          content: adr({
            id: "ADR-0002",
            status: "proposed",
            docStatus: "review",
            related: ["ADR-0001"],
            body: "ADR-0002 supersedes ADR-0001.",
          }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "proposed ADR prose claims active amendment or supersession");
  });

  for (const [format, subject] of [
    ["bold", "**ADR-0002**"],
    ["inline-code", "`ADR-0002`"],
  ]) {
    it(`rejects a proposed ADR whose ${format} subject ID claims active supersession`, () => {
      const root = createFixture({
        adrs: [
          {
            filename: "ADR-0001-accepted.md",
            content: adr({ id: "ADR-0001" }),
          },
          {
            filename: "ADR-0002-proposed.md",
            content: adr({
              id: "ADR-0002",
              status: "proposed",
              docStatus: "review",
              related: ["ADR-0001"],
              body: `${subject} supersedes ADR-0001.`,
            }),
          },
        ],
      });

      assertFailure(
        evaluateAdrGovernance(root),
        "proposed ADR prose claims active amendment or supersession",
      );
    });
  }

  it("allows conditional proposal language and ADR references in a design-note index row", () => {
    const root = createFixture({
      adrs: [
        { filename: "ADR-0001-accepted.md", content: adr({ id: "ADR-0001" }) },
        {
          filename: "ADR-0002-proposed.md",
          content: adr({
            id: "ADR-0002",
            status: "proposed",
            docStatus: "review",
            related: ["ADR-0001"],
            relationships: { proposes_amendments_to: ["ADR-0001"] },
            body: "If this ADR is accepted, amend ADR-0001.",
          }),
        },
      ],
      notes: [
        {
          filename: "DN-0001-adr-0002-detail.md",
          content: designNote({ parent: "ADR-0002" }),
        },
      ],
      indexRows: [
        "| [ADR-0001](ADR-0001-accepted.md) | accepted | Fixture |",
        "| [ADR-0002](ADR-0002-proposed.md) | proposed | Fixture |",
        "| [DN-0001](notes/DN-0001-adr-0002-detail.md) | proposed ADR-0002 | DARK |",
      ],
    });

    assert.deepEqual(evaluateAdrGovernance(root).failures, []);
  });

  it("rejects design notes without a valid parent ADR", () => {
    const root = createFixture({
      notes: [
        {
          filename: "DN-0001-orphan.md",
          content: designNote({ parent: "ADR-9999" }),
        },
      ],
    });

    assertFailure(evaluateAdrGovernance(root), "parent_adr ADR-9999 does not resolve");
  });

  it("rejects issuing the reserved ADR-0013 number", () => {
    const root = createFixture({
      adrs: [
        { filename: "ADR-0013-reserved.md", content: adr({ id: "ADR-0013" }) },
      ],
      indexRows: ["| [ADR-0013](ADR-0013-reserved.md) | accepted | Fixture |"],
    });

    assertFailure(evaluateAdrGovernance(root), "ADR-0013 is reserved and must never be issued");
  });
});
