#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const SHA = /^[0-9a-f]{40}$/;
const STABLE_SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const MANIFEST_PATH = ".release-please-manifest.json";
const CHANGELOG_PATH = "CHANGELOG.md";
const MAX_GIT_BLOB_BYTES = 8 * 1024 * 1024;

function fail(message) {
  throw new Error(`release metadata: ${message}`);
}

function exactSha(value, label) {
  if (!SHA.test(value ?? "")) fail(`${label} must be a lowercase 40-character SHA`);
  return value;
}

function exactUtf8(value, label, maximumBytes = MAX_GIT_BLOB_BYTES) {
  if (!Buffer.isBuffer(value) || value.length > maximumBytes) {
    fail(`${label} is unreadable or exceeds ${maximumBytes} bytes`);
  }
  const text = value.toString("utf8");
  if (!Buffer.from(text, "utf8").equals(value)) fail(`${label} must be valid UTF-8`);
  return text;
}

function manifestVersion(value, label) {
  const text = exactUtf8(value, label, 64 * 1024);
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    fail(`${label} must be valid JSON`);
  }
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== "object") {
    fail(`${label} must be an object with only the root package key`);
  }
  const keys = Object.keys(parsed);
  if (keys.length !== 1 || keys[0] !== "." || typeof parsed["."] !== "string") {
    fail(`${label} must contain exactly one string key named "."`);
  }
  const version = parsed["."];
  if (!STABLE_SEMVER.test(version)) fail(`${label} version must be canonical stable SemVer`);
  const canonical = `${JSON.stringify({ ".": version }, null, 2)}\n`;
  if (text !== canonical) {
    fail(`${label} must use canonical generated JSON bytes with a final LF`);
  }
  return version;
}

function compareSemver(left, right) {
  const leftParts = left.split(".").map(BigInt);
  const rightParts = right.split(".").map(BigInt);
  for (let index = 0; index < leftParts.length; index += 1) {
    if (leftParts[index] < rightParts[index]) return -1;
    if (leftParts[index] > rightParts[index]) return 1;
  }
  return 0;
}

function leadingChangelogVersion(value) {
  const text = exactUtf8(value, CHANGELOG_PATH);
  const heading = text.match(
    /^# Changelog\n\n## \[([^\]\r\n]+)\]\([^\r\n]+\) \(\d{4}-\d{2}-\d{2}\)\n/,
  );
  if (!heading) fail("CHANGELOG.md must begin with the canonical leading release heading");
  const version = heading[1];
  if (!STABLE_SEMVER.test(version)) {
    fail("CHANGELOG.md leading release version must be canonical stable SemVer");
  }
  return version;
}

export function validateReleaseMetadataBytes({ baseManifest, headManifest, headChangelog }) {
  const baseVersion = manifestVersion(baseManifest, `base ${MANIFEST_PATH}`);
  const headVersion = manifestVersion(headManifest, `head ${MANIFEST_PATH}`);
  if (compareSemver(headVersion, baseVersion) <= 0) {
    fail(`head version ${headVersion} must be strictly greater than base ${baseVersion}`);
  }
  const changelogVersion = leadingChangelogVersion(headChangelog);
  if (changelogVersion !== headVersion) {
    fail(`CHANGELOG.md leading version ${changelogVersion} must equal head manifest ${headVersion}`);
  }
  return Object.freeze({ baseVersion, headVersion, changelogVersion });
}

export function parseReleaseMetadataArgs(argv) {
  if (!Array.isArray(argv) || argv.length !== 4) {
    fail("usage: --base <40-hex-sha> --head <40-hex-sha>");
  }
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (key !== "--base" && key !== "--head") fail(`unknown argument: ${key}`);
    const name = key.slice(2);
    if (Object.hasOwn(values, name)) fail(`duplicate argument: ${key}`);
    values[name] = value;
  }
  if (!Object.hasOwn(values, "base") || !Object.hasOwn(values, "head")) {
    fail("usage: --base <40-hex-sha> --head <40-hex-sha>");
  }
  return Object.freeze({
    base: exactSha(values.base, "base"),
    head: exactSha(values.head, "head"),
  });
}

function git(repo, args) {
  try {
    return execFileSync(
      "git",
      ["-C", repo, "--no-pager", "-c", "core.hooksPath=/dev/null", ...args],
      {
        encoding: null,
        maxBuffer: MAX_GIT_BLOB_BYTES,
        stdio: ["ignore", "pipe", "pipe"],
        env: {
          ...process.env,
          GIT_CONFIG_NOSYSTEM: "1",
          GIT_CONFIG_GLOBAL: "/dev/null",
          GIT_CONFIG_SYSTEM: "/dev/null",
          GIT_TERMINAL_PROMPT: "0",
          GIT_ASKPASS: "/bin/false",
        },
      },
    );
  } catch {
    fail(`Git command failed: git ${args[0] ?? "<missing>"}`);
  }
}

function requireCommit(repo, sha, label) {
  try {
    git(repo, ["cat-file", "-e", `${sha}^{commit}`]);
  } catch {
    fail(`${label} commit is unavailable`);
  }
}

function readRegularBlob(repo, sha, file) {
  const listing = exactUtf8(
    git(repo, ["ls-tree", "-z", sha, "--", file]),
    `${sha}:${file} tree entry`,
    4096,
  );
  const expected = new RegExp(`^100644 blob [0-9a-f]{40}\\t${file.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\0$`);
  if (!expected.test(listing)) fail(`${sha}:${file} must be a regular mode-100644 blob`);
  return git(repo, ["show", `${sha}:${file}`]);
}

export function verifyReleaseMetadataRange({ repo = process.cwd(), base, head }) {
  const baseSha = exactSha(base, "base");
  const headSha = exactSha(head, "head");
  requireCommit(repo, baseSha, "base");
  requireCommit(repo, headSha, "head");
  return validateReleaseMetadataBytes({
    baseManifest: readRegularBlob(repo, baseSha, MANIFEST_PATH),
    headManifest: readRegularBlob(repo, headSha, MANIFEST_PATH),
    headChangelog: readRegularBlob(repo, headSha, CHANGELOG_PATH),
  });
}

function main() {
  const { base, head } = parseReleaseMetadataArgs(process.argv.slice(2));
  const result = verifyReleaseMetadataRange({ base, head });
  process.stdout.write(
    `release-metadata: ok base=${result.baseVersion} head=${result.headVersion} changelog=${result.changelogVersion}\n`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
