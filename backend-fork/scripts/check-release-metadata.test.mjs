import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

import {
  parseReleaseMetadataArgs,
  validateReleaseMetadataBytes,
  verifyReleaseMetadataRange,
} from "./check-release-metadata.mjs";
import { installGitFixtureEnvironment } from "./lib/git-fixture-environment.mjs";

installGitFixtureEnvironment();

const manifest = (version) => Buffer.from(`${JSON.stringify({ ".": version }, null, 2)}\n`);
const changelog = (previous, version) => Buffer.from(
  `# Changelog\n\n## [${version}](https://github.com/oyatie/console/compare/v${previous}...v${version}) (2026-08-15)\n\n### Bug Fixes\n\n* bounded release fixture\n`,
);

test("accepts canonical monotonic release metadata and numeric component ordering", () => {
  for (const [baseVersion, headVersion] of [
    ["0.3.5", "0.3.6"],
    ["0.9.9", "0.10.0"],
    ["0.10.9", "1.0.0"],
  ]) {
    assert.deepEqual(validateReleaseMetadataBytes({
      baseManifest: manifest(baseVersion),
      headManifest: manifest(headVersion),
      headChangelog: changelog(baseVersion, headVersion),
    }), {
      baseVersion,
      headVersion,
      changelogVersion: headVersion,
    });
  }
});

test("rejects equal or decreasing release versions", () => {
  for (const [baseVersion, headVersion] of [
    ["0.3.6", "0.3.6"],
    ["0.3.6", "0.3.5"],
    ["1.0.0", "0.99.99"],
  ]) {
    assert.throws(() => validateReleaseMetadataBytes({
      baseManifest: manifest(baseVersion),
      headManifest: manifest(headVersion),
      headChangelog: changelog(baseVersion, headVersion),
    }), /strictly greater/);
  }
});

test("rejects malformed, ambiguous, or noncanonical manifest bytes", () => {
  const validBase = manifest("0.3.5");
  for (const candidate of [
    Buffer.from("not json\n"),
    Buffer.from("[]\n"),
    Buffer.from('{".":"0.3.6","extra":"x"}\n'),
    Buffer.from('{".":"0.3.5",".":"0.3.6"}\n'),
    Buffer.from('{ ".": "0.3.6" }\n'),
    Buffer.from('{\r\n  ".": "0.3.6"\r\n}\r\n'),
    Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), manifest("0.3.6")]),
    Buffer.from([0xff, 0x00]),
  ]) {
    assert.throws(() => validateReleaseMetadataBytes({
      baseManifest: validBase,
      headManifest: candidate,
      headChangelog: changelog("0.3.5", "0.3.6"),
    }), /release metadata/);
  }
});

test("rejects noncanonical stable SemVer spellings", () => {
  for (const version of [
    "01.2.3",
    "1.02.3",
    "1.2.03",
    "v1.2.3",
    "1.2.3-alpha",
    "1.2.3+build",
    "+1.2.3",
    " 1.2.3",
  ]) {
    assert.throws(() => validateReleaseMetadataBytes({
      baseManifest: manifest("0.3.5"),
      headManifest: manifest(version),
      headChangelog: changelog("0.3.5", version),
    }), /stable SemVer/);
  }
});

test("requires the leading changelog release to agree with the head manifest", () => {
  for (const headChangelog of [
    Buffer.from("# Changelog\n\nNo release heading\n"),
    Buffer.from(`intro\n${changelog("0.3.5", "0.3.6")}`),
    changelog("0.3.5", "0.3.7"),
    Buffer.concat([changelog("0.3.5", "0.3.5"), changelog("0.3.5", "0.3.6")]),
  ]) {
    assert.throws(() => validateReleaseMetadataBytes({
      baseManifest: manifest("0.3.5"),
      headManifest: manifest("0.3.6"),
      headChangelog,
    }), /CHANGELOG|changelog/);
  }
});

test("CLI arguments require one exact base and head SHA", () => {
  const base = "a".repeat(40);
  const head = "b".repeat(40);
  assert.deepEqual(parseReleaseMetadataArgs(["--base", base, "--head", head]), { base, head });
  for (const argv of [
    [],
    ["--base", base],
    ["--base", base, "--head", "HEAD"],
    ["--base", base, "--base", head, "--head", head],
    ["--base", base, "--head", head, "--extra", "x"],
  ]) {
    assert.throws(() => parseReleaseMetadataArgs(argv), /usage|SHA|duplicate|unknown/);
  }
});

test("range verification reads exact commit blobs rather than mutable worktree files", () => {
  const repo = mkdtempSync(join(tmpdir(), "release-metadata-range-"));
  const git = (args) => {
    const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    return result.stdout.trim();
  };
  try {
    git(["init", "-q"]);
    git(["config", "user.email", "release-metadata@example.test"]);
    git(["config", "user.name", "release-metadata"]);
    writeFileSync(join(repo, ".release-please-manifest.json"), manifest("0.3.5"));
    writeFileSync(join(repo, "CHANGELOG.md"), changelog("0.3.4", "0.3.5"));
    git(["add", "."]);
    git(["commit", "-qm", "base release"]);
    const base = git(["rev-parse", "HEAD"]);

    writeFileSync(join(repo, ".release-please-manifest.json"), manifest("0.3.6"));
    writeFileSync(join(repo, "CHANGELOG.md"), changelog("0.3.5", "0.3.6"));
    git(["add", "."]);
    git(["commit", "-qm", "head release"]);
    const head = git(["rev-parse", "HEAD"]);

    writeFileSync(join(repo, ".release-please-manifest.json"), "not committed metadata\n");
    assert.deepEqual(verifyReleaseMetadataRange({ repo, base, head }), {
      baseVersion: "0.3.5",
      headVersion: "0.3.6",
      changelogVersion: "0.3.6",
    });
  } finally {
    rmSync(repo, { recursive: true, force: true });
  }
});

test("range verification rejects non-regular release metadata", () => {
  const repo = mkdtempSync(join(tmpdir(), "release-metadata-mode-"));
  const git = (args) => {
    const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    return result.stdout.trim();
  };
  try {
    git(["init", "-q"]);
    git(["config", "user.email", "release-metadata@example.test"]);
    git(["config", "user.name", "release-metadata"]);
    writeFileSync(join(repo, ".release-please-manifest.json"), manifest("0.3.5"));
    writeFileSync(join(repo, "CHANGELOG.md"), changelog("0.3.4", "0.3.5"));
    git(["add", "."]);
    git(["commit", "-qm", "base release"]);
    const base = git(["rev-parse", "HEAD"]);

    writeFileSync(join(repo, ".release-please-manifest.json"), manifest("0.3.6"));
    writeFileSync(join(repo, "CHANGELOG.md"), changelog("0.3.5", "0.3.6"));
    chmodSync(join(repo, ".release-please-manifest.json"), 0o755);
    git(["add", "."]);
    git(["commit", "-qm", "executable manifest"]);
    const head = git(["rev-parse", "HEAD"]);
    assert.throws(
      () => verifyReleaseMetadataRange({ repo, base, head }),
      /regular mode-100644 blob/,
    );
  } finally {
    rmSync(repo, { recursive: true, force: true });
  }
});
