import test from "node:test";
import assert from "node:assert/strict";
import { copyFile, mkdtemp, mkdir, readFile, symlink, unlink, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import {
  documentationArchiveMessage,
  documentationArchiveRef,
  validateDocumentationArchives,
} from "./console/validate-documentation-archive.mjs";
import { installGitFixtureEnvironment } from "./lib/git-fixture-environment.mjs";

installGitFixtureEnvironment();

const run = promisify(execFile);
const script = join(process.cwd(), "scripts/check-doc-links.mjs");
const generator = join(process.cwd(), "scripts/console/generate-documentation-manifest.mjs");
const archiveValidator = join(process.cwd(), "scripts/console/validate-documentation-archive.mjs");
const signaturePolicy = join(process.cwd(), "scripts/console/ssh-signature-policy.mjs");
const classVocabulary = [
  "current",
  "decision",
  "executable-contract",
  "evidence",
  "historical",
  "quarry",
];
const manifestFields = [
  "path",
  "class",
  "owner",
  "status",
  "replacement",
  "retention",
  "blob_sha",
  "archive_tag",
];
const excludedDocumentationPrefixes = [
  "buck-out/",
  "node_modules/",
  "target/",
  "third-party/",
  ".grok/",
];

const authorityPaths = {
  product: "docs/current/PRODUCT.md",
  roadmap: "docs/current/ROADMAP.md",
  delivery: "docs/current/DELIVERY.md",
};

function entry() {
  return {
    path: "README.md",
    class: "current",
    owner: "repository maintainers",
    status: "active",
    replacement: null,
    retention: "retain",
  };
}

function authority(concern, path = authorityPaths[concern]) {
  return {
    concern,
    path,
    class: "current",
    owner: "repository maintainers",
    status: "active",
    replacement: null,
    retention: "retain",
  };
}

function validReadme(extraAuthority = "") {
  return `# Entry

## Current authority

1. [Product](docs/current/PRODUCT.md)
2. [Roadmap](docs/current/ROADMAP.md)
3. [Delivery](docs/current/DELIVERY.md)
${extraAuthority}`;
}

function transition(path, status, replacement, retention) {
  return {
    path,
    class: "historical",
    owner: "repository maintainers",
    status,
    replacement,
    retention,
  };
}

function manifestRecord(record) {
  return {
    path: record.path,
    class: record.class,
    owner: record.owner,
    status: record.status,
    replacement: record.replacement,
    retention: record.retention,
    blob_sha: null,
    archive_tag: null,
  };
}

function validIndex(overrides = {}) {
  const entryRecord = entry();
  const authorities = Object.entries(authorityPaths).map(
    ([concern, path]) => authority(concern, path),
  );
  const transitions = [
    transition("SPEC.md", "redirect", "docs/current/PRODUCT.md", "one-release redirect"),
    transition("DESIGN.md", "redirect", "docs/current/PRODUCT.md", "one-release redirect"),
    transition(
      "docs/PIVOT-2026-07-28.md",
      "frozen",
      "docs/current/PRODUCT.md",
      "retain as historical reconciliation",
    ),
  ];
  return {
    schema_version: 2,
    coverage: "first-party-manifest",
    class_vocabulary: [...classVocabulary],
    future_full_manifest_fields: [...manifestFields],
    entry: entryRecord,
    authorities,
    transitions,
    documents: [
      manifestRecord(entryRecord),
      ...authorities.map(manifestRecord),
      ...transitions.map(manifestRecord),
    ].sort((left, right) => (left.path > right.path) - (left.path < right.path)),
    ...overrides,
  };
}

async function makeIndexedRepo(index, extraFiles = {}) {
  const root = await mkdtemp(join(tmpdir(), "doc-links-index-"));
  await run("git", ["init", "-q"], { cwd: root });
  const files = {
    "README.md": validReadme(),
    "SPEC.md": "# Product redirect\n",
    "DESIGN.md": "# Product redirect\n",
    "docs/PIVOT-2026-07-28.md": "# Historical reconciliation\n",
    "docs/current/PRODUCT.md": "# Product\n",
    "docs/current/ROADMAP.md": "# Roadmap\n",
    "docs/current/DELIVERY.md": "# Delivery\n",
    ...extraFiles,
  };
  for (const [path, contents] of Object.entries(files)) {
    await mkdir(join(root, path, ".."), { recursive: true });
    await writeFile(join(root, path), contents);
  }
  await run("git", ["add", "."], { cwd: root });
  for (const record of Array.isArray(index.documents) ? index.documents : []) {
    if (record?.blob_sha !== null) continue;
    const { stdout } = await run("git", ["ls-files", "--stage", "--", record.path], { cwd: root });
    const oid = stdout.trim().split(/\s+/)[1];
    if (oid) record.blob_sha = oid;
  }
  await mkdir(join(root, "docs"), { recursive: true });
  await writeFile(
    join(root, "docs/documentation-index.json"),
    `${JSON.stringify(index, null, 2)}\n`,
  );
  await run("git", ["add", "docs/documentation-index.json"], { cwd: root });
  return root;
}

function gitBlobOid(contents) {
  const bytes = Buffer.from(contents);
  return createHash("sha1")
    .update(`blob ${bytes.length}\0`)
    .update(bytes)
    .digest("hex");
}

async function makeSshAuthority(directory, name, principal) {
  const key = join(directory, name);
  await run("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-C", principal, "-f", key]);
  const publicKey = (await readFile(`${key}.pub`, "utf8")).trim().split(/\s+/).slice(0, 2).join(" ");
  const { stdout } = await run("ssh-keygen", ["-lf", `${key}.pub`, "-E", "sha256"]);
  return {
    key,
    principal,
    fingerprint: stdout.trim().split(/\s+/)[1],
    policy: `${principal} ${publicKey}\n`,
  };
}

async function makeArchiveFixture(options = {}) {
  const directory = await mkdtemp(join(tmpdir(), "doc-archive-contract-"));
  const principal = "archive-test@example.com";
  const trusted = await makeSshAuthority(directory, "trusted", principal);
  const forged = await makeSshAuthority(directory, "forged", principal);
  const authority = { format: "ssh", principal, fingerprint: trusted.fingerprint };
  const path = options.path ?? "docs/archive-history.md";
  const liveContents = options.liveContents ?? "# Archived history\n";
  const archivePath = options.archivePath ?? path;
  const archiveContents = options.archiveContents ?? liveContents;
  const record = {
    path,
    class: options.class ?? "historical",
    owner: "repository maintainers",
    status: options.status ?? "frozen",
    replacement: null,
    retention: "retain",
    blob_sha: gitBlobOid(liveContents),
    archive_tag: null,
  };
  const ref = documentationArchiveRef(record);
  const internalRef = options.internalRef ?? ref;
  const message = options.message ?? documentationArchiveMessage(record);
  const archiveEntryType = options.archiveEntryType ?? "blob";

  const archiveSource = join(directory, "archive-source");
  await mkdir(archiveSource);
  await run("git", ["init", "-q"], { cwd: archiveSource });
  await run("git", ["config", "user.name", "Archive Test"], { cwd: archiveSource });
  await run("git", ["config", "user.email", principal], { cwd: archiveSource });
  await mkdir(join(archiveSource, archivePath, ".."), { recursive: true });
  if (archiveEntryType === "gitlink") {
    await run("git", ["commit", "--allow-empty", "-q", "-m", "gitlink fixture target"], { cwd: archiveSource });
    const { stdout: gitlinkTargetOutput } = await run("git", ["rev-parse", "HEAD"], { cwd: archiveSource });
    await run(
      "git",
      ["update-index", "--add", "--cacheinfo", `160000,${gitlinkTargetOutput.trim()},${archivePath}`],
      { cwd: archiveSource },
    );
  } else {
    if (archiveEntryType === "symlink") {
      await symlink("archive-symlink-target.md", join(archiveSource, archivePath));
    } else {
      assert.equal(archiveEntryType, "blob");
      await writeFile(join(archiveSource, archivePath), archiveContents);
    }
    await run("git", ["--literal-pathspecs", "add", "--", archivePath], { cwd: archiveSource });
    if (options.archiveMode === "100755") {
      await run("git", ["update-index", "--chmod=+x", "--", archivePath], { cwd: archiveSource });
    }
  }
  await run("git", ["commit", "-q", "-m", "archive fixture"], { cwd: archiveSource });
  const { stdout: commitOutput } = await run("git", ["rev-parse", "HEAD"], { cwd: archiveSource });
  const commit = commitOutput.trim();
  const { stdout: treeOutput } = await run("git", ["rev-parse", "HEAD^{tree}"], { cwd: archiveSource });
  const target = options.targetType === "tree" ? treeOutput.trim() : commit;
  const shortInternalName = internalRef.slice("refs/tags/".length);
  if (options.tagKind === "lightweight") {
    await run("git", ["tag", shortInternalName, target], { cwd: archiveSource });
  } else if (options.tagKind === "unsigned") {
    await run("git", ["tag", "-a", "-m", message, shortInternalName, target], { cwd: archiveSource });
  } else {
    const signer = options.signer === "forged" ? forged : trusted;
    await run(
      "git",
      [
        "-c",
        "gpg.format=ssh",
        "-c",
        `user.signingkey=${signer.key}`,
        "tag",
        "-s",
        "-m",
        message,
        shortInternalName,
        target,
      ],
      { cwd: archiveSource },
    );
  }
  const { stdout: tagOutput } = await run("git", ["rev-parse", internalRef], { cwd: archiveSource });
  const tagOid = tagOutput.trim();
  record.archive_tag = { ref, tag_oid: tagOid };

  const remotePath = join(directory, "archive-remote.git");
  await run("git", ["init", "--bare", "-q", remotePath]);
  await run("git", ["config", "uploadpack.allowFilter", "true"], { cwd: remotePath });
  const remote = pathToFileURL(remotePath).href;
  if (options.publish !== false) {
    await run("git", ["push", "-q", remote, `${tagOid}:${ref}`], { cwd: archiveSource });
  }

  const index = validIndex();
  index.documents.push(record);
  index.documents.sort((left, right) => (left.path > right.path) - (left.path < right.path));
  const root = await makeIndexedRepo(index, {
    [path]: liveContents,
    ".github/trust/console.allowed_signers": trusted.policy,
  });
  return {
    archiveSource,
    authority,
    commit,
    directory,
    forged,
    index,
    liveContents,
    path,
    record,
    ref,
    remote,
    root,
    tagOid,
    trusted,
  };
}

async function stageFixtureIndex(fixture, transform) {
  const index = structuredClone(fixture.index);
  transform(index);
  await writeFile(
    join(fixture.root, "docs/documentation-index.json"),
    `${JSON.stringify(index, null, 2)}\n`,
  );
  await run("git", ["add", "docs/documentation-index.json"], { cwd: fixture.root });
  fixture.index = index;
}

function validateArchiveFixture(fixture) {
  return validateDocumentationArchives({
    root: fixture.root,
    testOnlyAuthority: fixture.authority,
    testOnlyRemoteUrl: fixture.remote,
  });
}

async function assertArchiveFixtureModes(fixture, archiveMode, archiveType) {
  const { stdout: liveEntryOutput } = await run(
    "git",
    ["ls-files", "--stage", "--", fixture.path],
    { cwd: fixture.root },
  );
  assert.equal(
    liveEntryOutput.trim(),
    `100644 ${fixture.record.blob_sha} 0\t${fixture.path}`,
  );
  const { stdout: archiveEntryOutput } = await run(
    "git",
    ["--literal-pathspecs", "ls-tree", fixture.commit, "--", fixture.path],
    { cwd: fixture.archiveSource },
  );
  const [metadata, path] = archiveEntryOutput.trim().split("\t");
  assert.equal(path, fixture.path);
  assert.match(metadata, new RegExp(`^${archiveMode} ${archiveType} [0-9a-f]{40}$`));
}

test("accepts local links and ignores external/anchor links", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await mkdir(join(root, "docs"));
  await writeFile(join(root, "README.md"), "[guide](docs/guide.md) [anchor](#top) [web](https://example.com)\n");
  await writeFile(join(root, "docs/guide.md"), "# Guide\n");
  await run(process.execPath, [script, root]);
});

test("fails with file and line for missing local target", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await writeFile(join(root, "README.md"), "[missing](docs/nope.md)\n");
  await assert.rejects(run(process.execPath, [script, root]), /README\.md:1: missing target: docs\/nope\.md/);
});

test("ignores fenced code", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await writeFile(join(root, "README.md"), "```md\n[missing](nope.md)\n```\n");
  await run(process.execPath, [script, root]);
});

test("ignores link-shaped examples inside inline code", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await writeFile(
    join(root, "README.md"),
    "Manifest text: `[AGENTS.md](AGENTS.md#task-selected-reasoning-lenses)`.\n",
  );
  await run(process.execPath, [script, root]);
});

test("still checks links beside inline code", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await writeFile(join(root, "README.md"), "`[example](ignored.md)` [missing](real-missing.md)\n");
  await assert.rejects(run(process.execPath, [script, root]), /missing target: real-missing\.md/);
});

test("ignores Buck output trees, including dangling artifact symlinks", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await writeFile(join(root, "README.md"), "# Clean tracked documentation\n");
  await mkdir(join(root, "buck-out"));
  await symlink(join(root, "missing-artifact.md"), join(root, "buck-out", "artifact.md"));
  await run(process.execPath, [script, root]);
});

test("rejects missing extensionless and reference-style targets", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-"));
  await writeFile(join(root, "README.md"), "[direct](missing)\n[ref]: absent-dir\n");
  await assert.rejects(run(process.execPath, [script, root]), /missing target: (missing|absent-dir)/);
});

test("accepts one tracked entry record plus three current authorities", async () => {
  const root = await makeIndexedRepo(validIndex());
  await run(process.execPath, [script, root]);
});

test("rejects unknown root completeness and document-manifest fields", async () => {
  for (const [field, value] of [["complete", true], ["manifest", []]]) {
    const root = await makeIndexedRepo(validIndex({ [field]: value }));
    await assert.rejects(
      run(process.execPath, [script, root]),
      new RegExp(`root record has unexpected field: ${field}`),
    );
  }
});

test("rejects unknown entry, authority, and transition record fields", async () => {
  const entryIndex = validIndex();
  entryIndex.entry.complete = true;
  const entryRoot = await makeIndexedRepo(entryIndex);
  await assert.rejects(
    run(process.execPath, [script, entryRoot]),
    /entry record has unexpected field: complete/,
  );

  const authorityIndex = validIndex();
  authorityIndex.authorities[0].documents = [];
  const authorityRoot = await makeIndexedRepo(authorityIndex);
  await assert.rejects(
    run(process.execPath, [script, authorityRoot]),
    /authority record has unexpected field: documents/,
  );

  const transitionIndex = validIndex({
    transitions: [{
      path: "SPEC.md",
      class: "historical",
      owner: "repository maintainers",
      status: "redirect",
      replacement: "docs/current/PRODUCT.md",
      retention: "one-release redirect",
      complete: true,
    }],
  });
  const transitionRoot = await makeIndexedRepo(transitionIndex, { "SPEC.md": "# Redirect\n" });
  await assert.rejects(
    run(process.execPath, [script, transitionRoot]),
    /transition record has unexpected field: complete/,
  );
});

test("rejects a fourth README current authority", async () => {
  const root = await makeIndexedRepo(validIndex(), {
    "README.md": validReadme("4. [Security](docs/current/SECURITY.md)\n"),
    "docs/current/SECURITY.md": "# Security\n",
  });
  await assert.rejects(
    run(process.execPath, [script, root]),
    /README\.md: current authority list must contain exactly/,
  );
});

test("rejects a prose declaration of a fourth README current authority", async () => {
  const root = await makeIndexedRepo(validIndex(), {
    "README.md": `${validReadme()}
The additional current security authority is [Security](docs/current/SECURITY.md).
`,
    "docs/current/SECURITY.md": "# Security\n",
  });
  await assert.rejects(
    run(process.execPath, [script, root]),
    /README\.md: current authority list must contain exactly/,
  );
});

test("rejects reordered README current authorities", async () => {
  const root = await makeIndexedRepo(validIndex(), {
    "README.md": `# Entry

## Current authority

1. [Roadmap](docs/current/ROADMAP.md)
2. [Product](docs/current/PRODUCT.md)
3. [Delivery](docs/current/DELIVERY.md)
`,
  });
  await assert.rejects(
    run(process.execPath, [script, root]),
    /README\.md: current authority list must contain exactly/,
  );
});

test("reads index and Markdown from the exact Git index tree, not unstaged worktree bytes", async () => {
  const root = await makeIndexedRepo(validIndex());

  await writeFile(join(root, "docs/documentation-index.json"), "{ not staged JSON\n");
  await writeFile(join(root, "README.md"), "[unstaged missing](does-not-exist.md)\n");
  await run(process.execPath, [script, root]);

  await writeFile(join(root, "docs/documentation-index.json"), "{ staged invalid JSON\n");
  await run("git", ["add", "docs/documentation-index.json"], { cwd: root });
  await writeFile(
    join(root, "docs/documentation-index.json"),
    `${JSON.stringify(validIndex(), null, 2)}\n`,
  );
  await assert.rejects(
    run(process.execPath, [script, root]),
    /documentation-index\.json: invalid JSON/,
  );
});

test("rejects a Git worktree with no documentation index", async () => {
  const root = await mkdtemp(join(tmpdir(), "doc-links-index-missing-"));
  await run("git", ["init", "-q"], { cwd: root });
  await writeFile(join(root, "README.md"), "# Entry\n");
  await run("git", ["add", "README.md"], { cwd: root });
  await assert.rejects(
    run(process.execPath, [script, root]),
    /docs\/documentation-index\.json: required in a Git worktree/,
  );
});

test("rejects an untracked or ignored documentation index", async () => {
  for (const ignored of [false, true]) {
    const root = await mkdtemp(join(tmpdir(), "doc-links-index-uncustodied-"));
    await run("git", ["init", "-q"], { cwd: root });
    const files = {
      "README.md": "# Entry\n",
      "docs/current/PRODUCT.md": "# Product\n",
      "docs/current/ROADMAP.md": "# Roadmap\n",
      "docs/current/DELIVERY.md": "# Delivery\n",
      "docs/documentation-index.json": `${JSON.stringify(validIndex(), null, 2)}\n`,
    };
    for (const [path, contents] of Object.entries(files)) {
      await mkdir(join(root, path, ".."), { recursive: true });
      await writeFile(join(root, path), contents);
    }
    if (ignored) {
      await writeFile(join(root, ".gitignore"), "docs/documentation-index.json\n");
      await run("git", ["add", ".gitignore"], { cwd: root });
    }
    await run("git", ["add", "README.md", "docs/current"], { cwd: root });
    await assert.rejects(
      run(process.execPath, [script, root]),
      /documentation-index\.json: index must be a regular blob in the exact Git index tree/,
    );
  }
});

test("cannot be redirected to an attacker-selected Git index", async () => {
  const root = await makeIndexedRepo(validIndex());
  const actualIndex = join(root, ".git/index");
  const alternateIndex = join(root, "alternate.index");
  await copyFile(actualIndex, alternateIndex);
  await run("git", ["rm", "--cached", "docs/documentation-index.json"], { cwd: root });

  await assert.rejects(
    run(process.execPath, [script, root], {
      env: { ...process.env, GIT_INDEX_FILE: alternateIndex },
    }),
    /documentation-index\.json: index must be a regular blob in the exact Git index tree/,
  );
});

test("rejects a tracked entry or authority symlink to uncustodied bytes", async () => {
  for (const [path, kind] of [
    ["README.md", "entry"],
    ["docs/current/PRODUCT.md", "authority"],
  ]) {
    const root = await makeIndexedRepo(validIndex());
    await writeFile(join(root, "external.md"), "# External untracked authority\n");
    await unlink(join(root, path));
    await symlink(join(root, "external.md"), join(root, path));
    await run("git", ["add", path], { cwd: root });
    await assert.rejects(
      run(process.execPath, [script, root]),
      new RegExp(`${kind} path must be a regular blob in the exact Git index tree`),
    );
  }
});

test("rejects an extensionless link whose tracked target is a symlink", async () => {
  const root = await makeIndexedRepo(validIndex());
  await writeFile(join(root, "outside.md"), "# Untracked outside bytes\n");
  await symlink(join(root, "outside.md"), join(root, "outside"));
  await writeFile(join(root, "README.md"), "[outside](outside)\n");
  await run("git", ["add", "README.md", "outside"], { cwd: root });

  await assert.rejects(
    run(process.execPath, [script, root]),
    /README\.md:1: missing target: outside/,
  );
});

test("rejects a missing current authority path", async () => {
  const index = validIndex();
  index.authorities[1].path = "docs/current/MISSING.md";
  const root = await makeIndexedRepo(index);
  await assert.rejects(
    run(process.execPath, [script, root]),
    /documentation-index\.json: authority path is not in the exact Git index tree: docs\/current\/MISSING\.md/,
  );
});

test("rejects duplicate concerns and admitted paths", async () => {
  const duplicateConcern = validIndex();
  duplicateConcern.authorities[1].concern = "product";
  const concernRoot = await makeIndexedRepo(duplicateConcern);
  await assert.rejects(run(process.execPath, [script, concernRoot]), /duplicate concern: product/);

  const duplicatePath = validIndex({
    transitions: [{
      path: "README.md",
      class: "historical",
      owner: "repository maintainers",
      status: "redirect",
      replacement: "docs/current/PRODUCT.md",
      retention: "one-release redirect",
    }],
  });
  const pathRoot = await makeIndexedRepo(duplicatePath);
  await assert.rejects(run(process.execPath, [script, pathRoot]), /duplicate admitted path: README\.md/);
});

test("rejects authority outside the allowed three concern/path pairs", async () => {
  const index = validIndex();
  index.authorities.push(authority("operations", "docs/current/OPERATIONS.md"));
  const root = await makeIndexedRepo(index, { "docs/current/OPERATIONS.md": "# Operations\n" });
  await assert.rejects(run(process.execPath, [script, root]), /unexpected authority concern: operations/);
});

test("rejects invalid redirect and frozen replacements", async () => {
  const badRedirect = validIndex({
    transitions: [{
      path: "SPEC.md",
      class: "historical",
      owner: "repository maintainers",
      status: "redirect",
      replacement: "docs/current/MISSING.md",
      retention: "one-release redirect",
    }],
  });
  const redirectRoot = await makeIndexedRepo(badRedirect, { "SPEC.md": "# Redirect\n" });
  await assert.rejects(run(process.execPath, [script, redirectRoot]), /redirect replacement is not an active authority/);

  const badFrozen = validIndex({
    transitions: [{
      path: "docs/PIVOT-2026-07-28.md",
      class: "historical",
      owner: "repository maintainers",
      status: "frozen",
      replacement: null,
      retention: "retain as historical reconciliation",
    }],
  });
  const frozenRoot = await makeIndexedRepo(badFrozen, { "docs/PIVOT-2026-07-28.md": "# Frozen\n" });
  await assert.rejects(run(process.execPath, [script, frozenRoot]), /frozen replacement is not an active authority/);
});

test("rejects untracked and ignored path admission", async () => {
  const transition = {
    path: "docs/private.md",
    class: "historical",
    owner: "repository maintainers",
    status: "redirect",
    replacement: "docs/current/PRODUCT.md",
    retention: "one-release redirect",
  };
  const untrackedRoot = await makeIndexedRepo(validIndex({ transitions: [transition] }));
  await writeFile(join(untrackedRoot, "docs/private.md"), "# Untracked\n");
  await assert.rejects(run(process.execPath, [script, untrackedRoot]), /admitted path is not in the exact Git index tree: docs\/private\.md/);

  const ignoredRoot = await makeIndexedRepo(validIndex({ transitions: [transition] }));
  await writeFile(join(ignoredRoot, ".gitignore"), "docs/private.md\n");
  await writeFile(join(ignoredRoot, "docs/private.md"), "# Ignored\n");
  await run("git", ["add", ".gitignore"], { cwd: ignoredRoot });
  await assert.rejects(run(process.execPath, [script, ignoredRoot]), /admitted path is not in the exact Git index tree: docs\/private\.md/);
});

test("rejects every premature complete-coverage claim", async () => {
  const root = await makeIndexedRepo(validIndex({ coverage: "complete" }), {
    "docs/reference.md": "# Reference\n",
  });
  await assert.rejects(
    run(process.execPath, [script, root]),
    /coverage complete is not admitted by Phase-A archive validation/,
  );
});

async function makeGeneratorRepo() {
  const index = validIndex();
  index.documents.push({
    path: "docs/CI-GATES.md",
    class: "executable-contract",
    owner: "repository maintainers",
    status: "active",
    replacement: null,
    retention: "retain",
    blob_sha: null,
    archive_tag: null,
  });
  index.documents.sort((left, right) => (left.path > right.path) - (left.path < right.path));
  const root = await makeIndexedRepo(index, { "docs/CI-GATES.md": "# CI gates\n" });
  await mkdir(join(root, "scripts/console"), { recursive: true });
  await copyFile(generator, join(root, "scripts/console/generate-documentation-manifest.mjs"));
  await copyFile(archiveValidator, join(root, "scripts/console/validate-documentation-archive.mjs"));
  await copyFile(signaturePolicy, join(root, "scripts/console/ssh-signature-policy.mjs"));
  await writeFile(
    join(root, "docs/documentation-manifest.seed.json"),
    `${JSON.stringify(index.documents, null, 2)}\n`,
  );
  await run(
    "git",
    ["add", "scripts/console", "docs/documentation-manifest.seed.json"],
    { cwd: root },
  );
  return root;
}

function literalStringArray(source, name) {
  const match = source.match(new RegExp(`const ${name} = \\[([\\s\\S]*?)\\n\\];`));
  assert.ok(match, `${name} literal must remain inspectable`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map((item) => item[1]);
}

test("accepts a green schema-v2 exact first-party manifest fixture", async () => {
  const root = await makeIndexedRepo(validIndex());
  await run(process.execPath, [script, root]);
});

test("duplicated first-party universe and class constants cannot drift", async () => {
  const [checkerSource, generatorSource] = await Promise.all([
    readFile(script, "utf8"),
    readFile(generator, "utf8"),
  ]);
  assert.deepEqual(
    literalStringArray(checkerSource, "excludedDocumentationPrefixes"),
    excludedDocumentationPrefixes,
  );
  assert.deepEqual(
    literalStringArray(generatorSource, "excludedDocumentationPrefixes"),
    excludedDocumentationPrefixes,
  );
  assert.deepEqual(literalStringArray(checkerSource, "classVocabulary"), classVocabulary);
  assert.deepEqual(literalStringArray(generatorSource, "classVocabulary"), classVocabulary);
});

test("all seven authority-slice records require exact document projections", async () => {
  for (let recordIndex = 0; recordIndex < 7; recordIndex += 1) {
    const index = validIndex();
    const records = [index.entry, ...index.authorities, ...index.transitions];
    records[recordIndex].class = recordIndex < 4 ? "historical" : "current";
    const root = await makeIndexedRepo(index);
    await assert.rejects(
      run(process.execPath, [script, root]),
      /document projection differs at class/,
    );
  }
});

test("P1 rejects a newly staged unclassified Markdown blob by path", async () => {
  const root = await makeGeneratorRepo();
  await writeFile(join(root, "docs/tmp-unclassified.md"), "# Unclassified\n");
  await run("git", ["add", "docs/tmp-unclassified.md"], { cwd: root });
  await assert.rejects(
    run(process.execPath, ["scripts/console/generate-documentation-manifest.mjs", "--check"], { cwd: root }),
    /docs\/tmp-unclassified\.md must have exactly one record/,
  );
});

test("P2 rejects staged Markdown blob drift until regeneration", async () => {
  const root = await makeGeneratorRepo();
  await writeFile(join(root, "docs/CI-GATES.md"), "# Changed CI gates\n");
  await run("git", ["add", "docs/CI-GATES.md"], { cwd: root });
  await assert.rejects(
    run(process.execPath, ["scripts/console/generate-documentation-manifest.mjs", "--check"], { cwd: root }),
    /docs\/CI-GATES\.md blob_sha does not match/,
  );
});

test("P3 rejects a missing full-manifest document record by path", async () => {
  const index = validIndex();
  index.documents = index.documents.filter((record) => record.path !== "DESIGN.md");
  const root = await makeIndexedRepo(index);
  await assert.rejects(
    run(process.execPath, [script, root]),
    /DESIGN\.md must have exactly one document record/,
  );
});

test("P4 keeps complete coverage fail-closed", async () => {
  const root = await makeIndexedRepo(validIndex({ coverage: "complete" }));
  await assert.rejects(
    run(process.execPath, [script, root]),
    /coverage complete is not admitted by Phase-A archive validation/,
  );
});

test("P5 rejects non-null archive tags that are not exact pinned objects", async () => {
  const index = validIndex();
  index.documents[0].archive_tag = "archive-v1";
  const root = await makeIndexedRepo(index);
  await assert.rejects(
    run(process.execPath, [script, root]),
    /archive_tag must be null or an exact pinned object/,
  );
});

test("P6 root field allowlist drift still rejects documents", async () => {
  const root = await makeIndexedRepo(validIndex());
  await mkdir(join(root, "scripts"), { recursive: true });
  const source = await readFile(script, "utf8");
  const mutated = source.replace(
    '  "transitions",\n  "documents",\n];',
    '  "transitions",\n];',
  );
  assert.notEqual(mutated, source);
  const mutatedScript = join(root, "scripts/check-doc-links.mjs");
  await writeFile(mutatedScript, mutated);
  await mkdir(join(root, "scripts/console"), { recursive: true });
  await copyFile(archiveValidator, join(root, "scripts/console/validate-documentation-archive.mjs"));
  await copyFile(signaturePolicy, join(root, "scripts/console/ssh-signature-policy.mjs"));
  await assert.rejects(
    run(process.execPath, [mutatedScript, root]),
    /root record has unexpected field: documents/,
  );
});

test("P7 write preserves a missing semantic class and check remains red", async () => {
  const root = await makeGeneratorRepo();
  const seedPath = join(root, "docs/documentation-manifest.seed.json");
  const seed = JSON.parse(await readFile(seedPath, "utf8"));
  delete seed[0].class;
  await writeFile(seedPath, `${JSON.stringify(seed, null, 2)}\n`);
  await run("git", ["add", "docs/documentation-manifest.seed.json"], { cwd: root });

  await run(
    process.execPath,
    ["scripts/console/generate-documentation-manifest.mjs", "--write"],
    { cwd: root },
  );
  const afterWrite = JSON.parse(await readFile(seedPath, "utf8"));
  assert.equal(Object.hasOwn(afterWrite[0], "class"), false);
  await assert.rejects(
    run(
      process.execPath,
      ["scripts/console/generate-documentation-manifest.mjs", "--check"],
      { cwd: root },
    ),
    /is missing class/,
  );
});

test("custom manifest diagnostics preserve the exact custom scope command", async () => {
  const root = await makeGeneratorRepo();
  const seed = JSON.parse(
    await readFile(join(root, "docs/documentation-manifest.seed.json"), "utf8"),
  );
  delete seed[0].class;
  await mkdir(join(root, "fixtures"), { recursive: true });
  await writeFile(
    join(root, "fixtures/custom scope.json"),
    `${JSON.stringify(seed, null, 2)}\n`,
  );
  await assert.rejects(
    run(
      process.execPath,
      [
        "scripts/console/generate-documentation-manifest.mjs",
        "--check",
        "--file",
        "fixtures/custom scope.json",
      ],
      { cwd: root },
    ),
    /Regenerate with: node scripts\/console\/generate-documentation-manifest\.mjs --write --file 'fixtures\/custom scope\.json'/,
  );
});

test("A1 archive validation keeps an all-null manifest offline", async () => {
  const root = await makeIndexedRepo(validIndex());
  const options = { root };
  Object.defineProperty(options, "testOnlyRemoteUrl", {
    get() { throw new Error("archive remote must not be read for an all-null manifest"); },
  });
  const validation = validateDocumentationArchives(options);
  assert.deepEqual(validation.failures, []);
  assert.equal(validation.telemetry.candidate_count, 0);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.equal(validation.telemetry.signer_policy_blob_oid, null);
});

test("A2 validates an exact freshly fetched signed annotated archive tag", async () => {
  const fixture = await makeArchiveFixture();
  const validation = validateArchiveFixture(fixture);
  assert.deepEqual(validation.failures, []);
  assert.equal(validation.telemetry.candidate_count, 1);
  assert.equal(validation.telemetry.validated_count, 1);
  assert.match(validation.telemetry.signer_policy_blob_oid, /^[0-9a-f]{40}$/);
  assert.deepEqual(validation.telemetry.results, [{
    path: fixture.path,
    ref: fixture.ref,
    tag_oid: fixture.tagOid,
    status: "validated",
  }]);
});

test("A3 rejects an archive tag signed by an untrusted SSH key", async () => {
  const fixture = await makeArchiveFixture({ signer: "forged" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /not signed exactly once by the indexed trusted SSH authority/);
});

test("A4 rejects an unsigned annotated archive tag", async () => {
  const fixture = await makeArchiveFixture({ tagKind: "unsigned" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /must contain exactly one SSH signature/);
});

test("A5 rejects a lightweight archive tag", async () => {
  const fixture = await makeArchiveFixture({ tagKind: "lightweight" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /not an annotated tag object/);
});

test("A6 rejects a remote archive ref moved away from its pinned tag object", async () => {
  const fixture = await makeArchiveFixture();
  await run(
    "git",
    ["push", "-q", fixture.remote, `+${fixture.commit}:${fixture.ref}`],
    { cwd: fixture.archiveSource },
  );
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /freshly fetched archive ref does not match pinned tag_oid/);
});

test("A7 rejects a pinned archive ref absent from the fresh remote", async () => {
  const fixture = await makeArchiveFixture({ publish: false });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /fresh fetch of the pinned archive ref failed/);
});

test("A8 rejects an annotated archive tag that targets a tree instead of a commit", async () => {
  const fixture = await makeArchiveFixture({ targetType: "tree" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /must target a commit directly|fresh fetch of the pinned archive ref failed/);
});

test("A9 rejects an archive commit missing the exact manifest path", async () => {
  const fixture = await makeArchiveFixture({ archivePath: "docs/a-different-history.md" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /must contain exactly the pinned document path/);
});

test("A10 rejects an archive commit whose document bytes differ", async () => {
  const fixture = await makeArchiveFixture({ archiveContents: "# Rewritten history\n" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /blob does not match the pinned blob_sha/);
});

test("A11 rejects an archive commit whose document mode is executable", async () => {
  const fixture = await makeArchiveFixture({ archiveMode: "100755" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /must be the exact 100644 document blob/);
});

test("A12 rejects two document records claiming one annotated tag object", async () => {
  const fixture = await makeArchiveFixture();
  const secondPath = "docs/archive-history-copy.md";
  await writeFile(join(fixture.root, secondPath), fixture.liveContents);
  await run("git", ["add", "--", secondPath], { cwd: fixture.root });
  await stageFixtureIndex(fixture, (index) => {
    const second = {
      ...fixture.record,
      path: secondPath,
      archive_tag: null,
    };
    second.archive_tag = {
      ref: documentationArchiveRef(second),
      tag_oid: fixture.tagOid,
    };
    index.documents.push(second);
    index.documents.sort((left, right) => (left.path > right.path) - (left.path < right.path));
  });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /duplicates tag_oid claimed by/);
});

test("A13 rejects malformed pins, derived-ref drift, OID drift, and internal tag-name drift", async () => {
  const fixture = await makeArchiveFixture();
  const original = structuredClone(fixture.index);
  const cases = [
    ["archive-v1", /must be null or an exact pinned object/],
    [{ ref: fixture.ref }, /is missing tag_oid/],
    [{ ref: fixture.ref, tag_oid: fixture.tagOid, extra: true }, /has unexpected field: extra/],
    [{ ref: `${fixture.ref}-wrong`, tag_oid: fixture.tagOid }, /ref does not match the derived/],
    [{ ref: fixture.ref, tag_oid: fixture.tagOid.toUpperCase() }, /tag_oid must be a 40-character lowercase/],
  ];
  for (const [archiveTag, expected] of cases) {
    fixture.index = structuredClone(original);
    await stageFixtureIndex(fixture, (index) => {
      index.documents.find((record) => record.path === fixture.path).archive_tag = archiveTag;
    });
    const validation = validateArchiveFixture(fixture);
    assert.match(validation.failures.join("\n"), expected);
  }

  const wrongInternalRef = `refs/tags/archive/documentation/v1/${"f".repeat(64)}`;
  const internalNameFixture = await makeArchiveFixture({ internalRef: wrongInternalRef });
  assert.notEqual(internalNameFixture.ref, wrongInternalRef);
  const internalNameValidation = validateArchiveFixture(internalNameFixture);
  assert.match(internalNameValidation.failures.join("\n"), /internal name does not match the pinned ref/);
});

test("A14 rejects a signed tag whose message is not canonical archive metadata", async () => {
  const fixture = await makeArchiveFixture({ message: "not canonical archive metadata" });
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /message does not match canonical archive metadata/);
});

test("A15 treats hostile paths as literals and never executes archived document bytes", async () => {
  const marker = join(
    tmpdir(),
    `console-doc-archive-must-not-exist-${process.pid}-${Date.now()}-${Math.random()}`,
  );
  const hostilePath = ":(glob)docs/[archive]*;$(touch owned).md";
  const contents = `#!/bin/sh\nprintf compromised > ${JSON.stringify(marker)}\n`;
  const fixture = await makeArchiveFixture({ path: hostilePath, liveContents: contents });
  const validation = validateArchiveFixture(fixture);
  assert.deepEqual(validation.failures, []);
  assert.equal(validation.telemetry.validated_count, 1);
  await assert.rejects(readFile(marker), { code: "ENOENT" });
});

test("A16 rejects indexed trust drift, replacement objects, and stale local archive refs", async () => {
  const policyFixture = await makeArchiveFixture();
  await writeFile(
    join(policyFixture.root, ".github/trust/console.allowed_signers"),
    policyFixture.forged.policy,
  );
  await run(
    "git",
    ["add", ".github/trust/console.allowed_signers"],
    { cwd: policyFixture.root },
  );
  const policyValidation = validateArchiveFixture(policyFixture);
  assert.match(policyValidation.failures.join("\n"), /policy fingerprint is not trusted/);

  const staleFixture = await makeArchiveFixture({ publish: false });
  await run(
    "git",
    ["-c", "protocol.file.allow=always", "fetch", "-q", "--no-tags", staleFixture.archiveSource, `+${staleFixture.ref}:${staleFixture.ref}`],
    { cwd: staleFixture.root },
  );
  const staleValidation = validateArchiveFixture(staleFixture);
  assert.equal(staleValidation.telemetry.validated_count, 0);
  assert.match(staleValidation.failures.join("\n"), /fresh fetch of the pinned archive ref failed/);

  const replacementRoot = await makeGeneratorRepo();
  const indexPath = "docs/documentation-index.json";
  const { stdout: safeStage } = await run(
    "git",
    ["ls-files", "--stage", "--", indexPath],
    { cwd: replacementRoot },
  );
  const safeIndexOid = safeStage.trim().split(/\s+/)[1];
  const replacedIndex = JSON.parse(await readFile(join(replacementRoot, indexPath), "utf8"));
  replacedIndex.documents[0].archive_tag = "attacker-hidden-non-null-pin";
  await writeFile(join(replacementRoot, indexPath), `${JSON.stringify(replacedIndex, null, 2)}\n`);
  await run("git", ["add", indexPath], { cwd: replacementRoot });
  const { stdout: replacedStage } = await run(
    "git",
    ["ls-files", "--stage", "--", indexPath],
    { cwd: replacementRoot },
  );
  const replacedIndexOid = replacedStage.trim().split(/\s+/)[1];
  assert.notEqual(replacedIndexOid, safeIndexOid);
  await run("git", ["replace", replacedIndexOid, safeIndexOid], { cwd: replacementRoot });

  await assert.rejects(
    run(process.execPath, [script, replacementRoot]),
    /archive_tag must be null or an exact pinned object/,
  );
  await assert.rejects(
    run(
      process.execPath,
      ["scripts/console/generate-documentation-manifest.mjs", "--check"],
      { cwd: replacementRoot },
    ),
    /generated bytes or preserved semantics are stale/,
  );
  const replacementValidation = validateDocumentationArchives({ root: replacementRoot });
  assert.match(
    replacementValidation.failures.join("\n"),
    /archive_tag must be null or an exact pinned object/,
  );
});

test("A17 rejects an archive-tree symlink while the live manifest path remains 100644", async () => {
  const fixture = await makeArchiveFixture({ archiveEntryType: "symlink" });
  await assertArchiveFixtureModes(fixture, "120000", "blob");
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /must be the exact 100644 document blob/);
});

test("A18 rejects an archive-tree gitlink while the live manifest path remains 100644", async () => {
  const fixture = await makeArchiveFixture({ archiveEntryType: "gitlink" });
  await assertArchiveFixtureModes(fixture, "160000", "commit");
  const validation = validateArchiveFixture(fixture);
  assert.equal(validation.telemetry.validated_count, 0);
  assert.match(validation.failures.join("\n"), /must be the exact 100644 document blob/);
});
