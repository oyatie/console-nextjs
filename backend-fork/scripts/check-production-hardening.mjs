#!/usr/bin/env node
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  allWorkflowCommands,
  directExecutable,
  shellCommandTokens,
} from "./lib/ci-workflow-executables.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function read(path) {
  const abs = resolve(root, path);
  return existsSync(abs) ? readFileSync(abs, "utf8") : "";
}

function createResult() {
  return { failures: [], passes: [] };
}

function appendResult(target, source) {
  target.passes.push(...source.passes);
  target.failures.push(...source.failures);
}

function requirement(result, ok, passMessage, failureMessage) {
  if (ok) {
    result.passes.push(passMessage);
  } else {
    result.failures.push(failureMessage);
  }
}

function requirePresentText(result, readText, path, label = path) {
  const text = readText(path);
  requirement(
    result,
    text.trim().length > 0,
    `${label}: present`,
    `${label}: missing or empty (${path})`,
  );
  return text;
}

function requireIncludesInText(result, path, text, needle, label) {
  requirement(
    result,
    text.includes(needle),
    label,
    `${label}: ${path} must include ${JSON.stringify(needle)}`,
  );
}

function requireRegexInText(
  result,
  path,
  text,
  regex,
  label,
  failureDetail = `must match ${regex}`,
) {
  requirement(
    result,
    regex.test(text),
    label,
    `${label}: ${path} ${failureDetail}`,
  );
}

function requireTextIncludes(result, readText, path, needle, label) {
  requireIncludesInText(result, path, readText(path), needle, label);
}

function requirePackageScript(result, readText, name) {
  const pkgText = readText("package.json");
  let pkg;
  try {
    pkg = JSON.parse(pkgText);
  } catch (error) {
    result.failures.push(
      `package script ${name}: package.json must be valid JSON (${error.message})`,
    );
    return;
  }

  const scripts = pkg.scripts && typeof pkg.scripts === "object" ? pkg.scripts : null;
  const scriptBody = scripts && Object.hasOwn(scripts, name) ? scripts[name] : undefined;
  requirement(
    result,
    typeof scriptBody === "string" && scriptBody.length > 0,
    `package script ${name}: ${scriptBody}`,
    `package script ${name}: missing from package.json scripts`,
  );
}

function stripYamlScalar(value) {
  if (value === undefined) return undefined;
  const withoutComment = String(value)
    .replace(/\s+#.*$/, "")
    .trim();
  if (
    (withoutComment.startsWith('"') && withoutComment.endsWith('"')) ||
    (withoutComment.startsWith("'") && withoutComment.endsWith("'"))
  ) {
    return withoutComment.slice(1, -1);
  }
  return withoutComment;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function extractYamlScalar(text, key) {
  const match = text.match(
    new RegExp(`^\\s*${escapeRegExp(key)}:\\s*(.+?)\\s*$`, "m"),
  );
  return stripYamlScalar(match?.[1]);
}

function extractJson6902PatchScalar(text, pointer) {
  const lines = text.split(/\r?\n/);
  const pathMatcher = new RegExp(`^\\s*path:\\s*${escapeRegExp(pointer)}\\s*$`);
  for (let index = 0; index < lines.length; index += 1) {
    if (!pathMatcher.test(lines[index])) continue;
    for (
      let valueIndex = index + 1;
      valueIndex < lines.length;
      valueIndex += 1
    ) {
      if (/^\s*-\s*op\s*:/.test(lines[valueIndex])) break;
      const valueMatch = lines[valueIndex].match(/^\s*value:\s*(.*)$/);
      if (valueMatch) return stripYamlScalar(valueMatch[1]);
    }
  }
  return undefined;
}

function json6902PatchHasPath(text, pointer) {
  return new RegExp(`^\\s*path:\\s*${escapeRegExp(pointer)}\\s*$`, "m").test(
    text,
  );
}

function parsePositiveInteger(value) {
  const parsed = Number.parseInt(String(value ?? ""), 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : undefined;
}

function formatScalar(value) {
  return value === undefined || value === ""
    ? "missing"
    : JSON.stringify(value);
}

const SMTP_NON_SECRET_KEYS = Object.freeze([
  "CONSOLE_EMAIL_SMTP_HOST",
  "CONSOLE_EMAIL_SMTP_PORT",
  "CONSOLE_EMAIL_FROM",
  "CONSOLE_EMAIL_FROM_NAME",
]);

const SMTP_SECRET_KEYS = Object.freeze([
  "CONSOLE_EMAIL_SMTP_USERNAME",
  "CONSOLE_EMAIL_SMTP_PASSWORD",
]);

const SMTP_STUB_MODE_KEY = "CONSOLE_EMAIL_STUB_MODE";
const SMTP_ALLOWED_STUB_MODES = Object.freeze([
  "local",
  "dev",
  "development",
  "test",
  "e2e",
]);

const SMTP_WORKLOADS = Object.freeze([
  { label: "console-app", path: "deploy/apps/console/base/backend.yaml" },
  { label: "console-worker", path: "deploy/apps/console/base/worker.yaml" },
]);

const CNPG_OCI_CHECKSUM_ENV_NAMES = Object.freeze([
  "AWS_REQUEST_CHECKSUM_CALCULATION",
  "AWS_RESPONSE_CHECKSUM_VALIDATION",
]);

function stripInlineHashComment(line) {
  let quote = "";
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (char === "\\") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (char === quote) quote = "";
      continue;
    }
    if (char === '"' || char === "'" || char === "`") {
      quote = char;
      continue;
    }
    if (char === "#" && (index === 0 || /\s/.test(line[index - 1]))) {
      return line.slice(0, index);
    }
  }
  return line;
}

function stripHashComments(text) {
  return text.split(/\r?\n/).map(stripInlineHashComment).join("\n");
}

function activeYamlScalarKeys(text, keys) {
  const stripped = stripHashComments(text);
  return keys.filter((key) => {
    const value = extractYamlScalar(stripped, key);
    return value !== undefined && value !== "";
  });
}

function extractYamlMappingBlock(text, key, keyIndent) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const matcher = new RegExp(`^\\s{${keyIndent}}${escapeRegExp(key)}:\\s*$`);
  for (let index = 0; index < lines.length; index += 1) {
    if (!matcher.test(lines[index])) continue;
    const block = [lines[index]];
    for (
      let blockIndex = index + 1;
      blockIndex < lines.length;
      blockIndex += 1
    ) {
      const line = lines[blockIndex];
      if (line.trim() === "") {
        block.push(line);
        continue;
      }
      if (countLeadingSpaces(line) <= keyIndent) break;
      block.push(line);
    }
    return block.join("\n");
  }
  return "";
}

function countLeadingSpaces(line) {
  return line.match(/^\s*/)?.[0].length ?? 0;
}

function normalizeCommandText(text) {
  return text
    .replace(/\\\r?\n\s*/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function extractGithubWorkflowRunBlocks(text) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const blocks = [];
  for (let index = 0; index < lines.length; index += 1) {
    const runMatch = lines[index].match(/^(\s*)(?:-\s*)?run:\s*(.*)$/);
    if (!runMatch) continue;
    const indent = runMatch[1].length;
    const scalar = runMatch[2].trim();
    if (/^[|>][+-]?$/.test(scalar)) {
      const blockLines = [];
      for (
        let blockIndex = index + 1;
        blockIndex < lines.length;
        blockIndex += 1
      ) {
        const line = lines[blockIndex];
        if (line.trim() === "") {
          blockLines.push("");
          continue;
        }
        if (countLeadingSpaces(line) <= indent) break;
        blockLines.push(line.slice(Math.min(line.length, indent + 2)));
      }
      blocks.push(blockLines.join("\n"));
    } else if (scalar.length > 0) {
      blocks.push(stripYamlScalar(scalar));
    }
  }
  return blocks.map(normalizeCommandText);
}

function workflowHasRun(text, predicates) {
  const blocks = extractGithubWorkflowRunBlocks(text);
  return blocks.some((block) =>
    predicates.every((predicate) => predicate.test(block)),
  );
}

// ---------------------------------------------------------------------------
// Fail-closed wrapper-argv0 gate (console-ry4f mechanism replacement).
//
// WHY THIS IS A MECHANISM, NOT AN ARGV0 STRIP
//
// The earlier ExactActiveRun strip (echo/printf, then bash -c, then eval, then
// shell compound prefixes) was a denylist of narrative argv0s applied as
// strip-and-continue: each residual (ta90 -> pwys -> 8rr1 -> c236) was "one more
// argv0 spelling". A strip keeps matching whatever survives the denylist, so it can
// never be closed; there is always another wrapper spelling.
//
// This gate inverts the question. The committed inventory
// docs/program/executed-tests-baseline.json names the SOURCES whose tests must
// execute (test_attribute_baseline); the real commands that execute them are the
// binary itself (cargo, npm, node, tools/buck2, cargo-audit, ...). A CI/check
// invocation whose argv0 is `source`, `.`, or `timeout` does not execute its target
// directly — it delegates to an unscanned file or a timed wrapper — so it cannot
// stand in for the real binary/test command. Presence of that wrapper argv0 is a
// finding, not something to look through: FAIL CLOSED.
// ---------------------------------------------------------------------------
const WRAPPER_ARGV0S = new Set(["source", ".", "timeout"]);

// Real binaries this repository's CI uses to execute tests/checks/security scans.
// This is an ALLOWLIST of the executor — the inverse of a narrative denylist. An
// invocation that names one of these through a wrapper is refused; a name off the
// list is not a test/check executor the wrapper gate is meant to defend.
const TEST_CHECK_EXECUTORS = new Set([
  "cargo",
  "cargo-audit",
  "cargo-deny",
  "npm",
  "npx",
  "yarn",
  "node",
  "python3",
  "pytest",
  "buck2",
  "trivy",
  "cosign",
]);

// Security-gating executors whose exit status IS the enforcement: a masked status
// (`||`, `&&`, a pipeline, backgrounding, or `set +e`) would make their failure
// non-blocking. General executors (cargo run &, npm audit inside if/else) legitimately
// mask status, so the masked-status check is scoped to this set only.
const MUST_GATE_EXECUTORS = new Set([
  "cargo-audit",
  "cargo-deny",
  "trivy",
  "cosign",
]);

function commandBasename(argv0) {
  if (!argv0) return "";
  return argv0.includes("/") ? argv0.slice(argv0.lastIndexOf("/") + 1) : argv0;
}

/** Basename of a test/check executor, ignoring a shell/python/js script extension. */
function executorBasename(token) {
  let base = commandBasename(token);
  for (const ext of [".sh", ".mjs", ".js", ".py"]) {
    if (base.endsWith(ext)) base = base.slice(0, -ext.length);
  }
  return base;
}

/**
 * `timeout [OPTION] DURATION COMMAND [ARG]...` wraps COMMAND. Return the COMMAND
 * tokens so its argv0 can be classified. A best-effort option parse is enough: an
 * invocation that cannot be classified has no wrapped executor and is not flagged.
 */
function timeoutWrappedTokens(tokens) {
  let index = 1;
  while (index < tokens.length) {
    const token = tokens[index];
    if (token === "--") {
      index += 1;
      break;
    }
    if (
      token === "-k" ||
      token === "--kill-after" ||
      token === "-s" ||
      token === "--signal"
    ) {
      index += 2;
      continue;
    }
    if (token.startsWith("--kill-after=") || token.startsWith("--signal=")) {
      index += 1;
      continue;
    }
    if (token.startsWith("-") && token !== "-") {
      index += 1;
      continue;
    }
    break;
  }
  if (index >= tokens.length) return [];
  index += 1; // DURATION
  return tokens.slice(index);
}

// Shell control-flow keywords can prefix a wrapper without changing what it runs:
// `if timeout 30 ...; then` still runs `timeout`, and `! timeout ...` still runs
// `timeout`. The wrapper gate classifies the argv0 AFTER these transparent prefixes so
// a wrapper cannot hide behind a compound-command head or a negation.
const CONTROL_FLOW_PREFIXES = new Set([
  "if",
  "then",
  "elif",
  "else",
  "fi",
  "while",
  "until",
  "do",
  "done",
  "for",
  "in",
  "select",
  "case",
  "esac",
  "!",
  "{",
  "}",
  "(",
  ")",
  "[[",
  "]]",
  // `builtin` executes the named shell builtin with its arguments (bash `help builtin`),
  // so `builtin source ...` still invokes `source` and must classify the same way.
  "builtin",
]);

function stripControlFlowPrefix(tokens) {
  let index = 0;
  while (index < tokens.length && CONTROL_FLOW_PREFIXES.has(tokens[index])) {
    index += 1;
  }
  return tokens.slice(index);
}

/**
 * `source [--] FILENAME` / `. [--] FILENAME` wraps FILENAME. Skip the `--` option
 * terminator (bash accepts it) before selecting the sourced target so `source -- file`
 * is classified the same as `source file`.
 */
function sourceWrappedTokens(tokens) {
  let index = 1;
  if (tokens[index] === "--") index += 1;
  return tokens.slice(index, index + 1);
}

/**
 * Alternate `directExecutable` (assignment / `command` / `env` stripping) with the
 * control-flow-prefix strip until the argv0 stabilizes, so `if env timeout ...`
 * reduces to `timeout` instead of stopping at `env` or `if`.
 */
function classifyArgv0(tokens) {
  let current = tokens;
  for (let round = 0; round < 16; round += 1) {
    const next = stripControlFlowPrefix(directExecutable(current).tokens);
    if (next.length === current.length && next.every((token, index) => token === current[index])) {
      return next;
    }
    current = next;
  }
  return current;
}

/** Basename of the direct executor after transparent-prefix stripping. */
function directExecutorName(tokens) {
  return executorBasename(directExecutable(tokens).tokens[0]);
}

// Shells that can execute a `-c <command>` string; a protected executor invoked through
// one of these must still be classified, not hidden behind the shell name.
const NESTED_SHELLS = new Set(["bash", "sh", "zsh", "dash", "ksh"]);

/** Strip one pair of matching YAML flow scalar quotes from a value. */
function unquoteYamlScalar(value) {
  if (value.length < 2) return value;
  const first = value[0];
  const last = value[value.length - 1];
  if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
    return value.slice(1, -1);
  }
  return value;
}

/** Inspect a `bash -c "<command>"` payload for masked protected executors. */
function nestedShellFindings(tokens, segment) {
  const findings = [];
  const argv0 = classifyArgv0(tokens);
  const shell = commandBasename(argv0[0]);
  if (!NESTED_SHELLS.has(shell)) return findings;
  // `-c` may be combined with other invocation flags (`bash -ec "..."`), so match any
  // single-dash option token that contains a lowercase `c`.
  const cIndex = argv0.findIndex(
    (token, index) => index > 0 && token.startsWith("-") && !token.startsWith("--") && token !== "-" && token.includes("c"),
  );
  if (cIndex < 0 || cIndex + 1 >= argv0.length) return findings;
  const commandString = argv0[cIndex + 1];
  for (const surface of shellCommandTokens(commandString)) {
    if (surface.malformed) {
      findings.push({ kind: "malformed", segment });
      continue;
    }
    const masked = surface.tokens.some((token) => ["||", "&&", "|", "&", ";"].includes(token));
    for (const token of surface.tokens) {
      const name = executorBasename(token);
      if (MUST_GATE_EXECUTORS.has(name) && masked) {
        findings.push({ kind: "nested-shell-masked", wrapped: name, segment });
      }
    }
  }
  return findings;
}

/**
 * Scan every workflow run step (gating and non-gating) for wrapper-gate violations:
 *   - malformed     : an unparseable command surface, refused instead of skipped;
 *   - wrapper       : `source` / `.` / `timeout` argv0 around a test/check executor;
 *   - unclassifiable: a wrapper whose target is a variable/substitution, fail closed;
 *   - non-gating    : a protected executor in a step `if: false` / continue-on-error drops;
 *   - shell         : a protected executor whose step overrides the runner shell;
 *   - masked-status : a must-gate executor whose exit status is masked;
 *   - nested-shell  : a must-gate executor masked inside `bash -c ...`;
 *   - no-structure  : a workflow whose `jobs:` cannot be extracted.
 * An empty list means every protected executor runs directly and gates.
 */
function workflowWrapperInvocations(workflowText) {
  const findings = [];
  const commands = allWorkflowCommands(workflowText);
  if (/^jobs:/m.test(workflowText) && commands.length === 0) {
    findings.push({ kind: "no-structure", segment: "no run commands extracted" });
    return findings;
  }
  for (const command of commands) {
    const segment = command.tokens.join(" ");
    if (command.malformed) {
      findings.push({ kind: "malformed", segment });
      continue;
    }
    const argv0 = classifyArgv0(command.tokens);
    const basename = commandBasename(argv0[0]);

    if (WRAPPER_ARGV0S.has(basename)) {
      const wrappedTokens =
        basename === "timeout"
          ? timeoutWrappedTokens(argv0)
          : sourceWrappedTokens(argv0);
      const wrappedRaw = directExecutable(wrappedTokens).tokens[0] ?? "";
      const wrapped = executorBasename(wrappedRaw);
      if (TEST_CHECK_EXECUTORS.has(wrapped)) {
        findings.push({ kind: "wrapper", argv0: basename, wrapped, segment });
        continue;
      }
      if (wrappedRaw.startsWith("$") || wrappedRaw.includes("`")) {
        findings.push({ kind: "unclassifiable-wrapper", argv0: basename, segment });
        continue;
      }
    }

    const protectedName = directExecutorName(argv0);
    if (TEST_CHECK_EXECUTORS.has(protectedName)) {
      if (!command.gating) {
        findings.push({ kind: "non-gating", wrapped: protectedName, segment });
      }
      const effectiveShell = command.shell === null ? null : unquoteYamlScalar(command.shell);
      if (effectiveShell !== null && effectiveShell !== "bash") {
        findings.push({ kind: "shell", wrapped: protectedName, shell: effectiveShell, segment });
      }
      const mustGate = MUST_GATE_EXECUTORS.has(protectedName)
        || (protectedName === "node" && /\bcheck-node-audit-exceptions\.mjs\b/.test(segment));
      // A leading condition keyword (`if`/`while`/`until`/`!`/`elif`/`case`) means the
      // executor runs as a condition whose failure does not fail the step, even when the
      // `then`/`fi` live on other lines.
      const conditionContext = ["if", "while", "until", "elif", "!", "case"].includes(
        directExecutable(command.tokens).tokens[0],
      );
      if (mustGate && (command.controlFlow || conditionContext)) {
        findings.push({ kind: "masked-status", wrapped: protectedName, segment });
      }
    }

    // A must-gate executor hidden inside a function/group body (`sign() { cosign ... ||
    // true; }`) is masked even though the segment argv0 is the function name. The
    // tokenizer keeps `sign()` as one token, so match a trailing `()` plus a `{`.
    if (
      command.controlFlow
      && command.tokens.some((token) => token.endsWith("()"))
      && command.tokens.includes("{")
    ) {
      for (const token of command.tokens) {
        const name = executorBasename(token);
        if (MUST_GATE_EXECUTORS.has(name)) {
          findings.push({ kind: "masked-status", wrapped: name, segment });
          break;
        }
      }
    }

    findings.push(...nestedShellFindings(argv0, segment));
  }
  return findings;
}

function wrapperFindingMessage(workflowPath, finding) {
  if (finding.kind === "wrapper") {
    return `wrapper-argv0 gate: ${workflowPath} must run ${finding.wrapped} as a real binary, not through the ${finding.argv0} wrapper (${finding.segment})`;
  }
  if (finding.kind === "malformed") {
    return `wrapper-argv0 gate: ${workflowPath} contains an unparseable run command that must be fixed or removed rather than skipped (${finding.segment})`;
  }
  if (finding.kind === "non-gating") {
    return `wrapper-argv0 gate: ${workflowPath} must run ${finding.wrapped} in a gating step, not one dropped by if: false or continue-on-error (${finding.segment})`;
  }
  if (finding.kind === "shell") {
    return `wrapper-argv0 gate: ${workflowPath} must run ${finding.wrapped} with the default runner shell, not the ${finding.shell} override (${finding.segment})`;
  }
  if (finding.kind === "masked-status") {
    return `wrapper-argv0 gate: ${workflowPath} must not mask the exit status of ${finding.wrapped} with ||, &&, a pipeline, backgrounding, or set +e (${finding.segment})`;
  }
  if (finding.kind === "unclassifiable-wrapper") {
    return `wrapper-argv0 gate: ${workflowPath} must run test/check binaries directly, not through the ${finding.argv0} wrapper around an unclassifiable target (${finding.segment})`;
  }
  if (finding.kind === "nested-shell-masked") {
    return `wrapper-argv0 gate: ${workflowPath} must not mask the exit status of ${finding.wrapped} inside a nested shell -c command (${finding.segment})`;
  }
  if (finding.kind === "no-structure") {
    return `wrapper-argv0 gate: ${workflowPath} must extract a runnable jobs structure rather than scan an empty command set (${finding.segment})`;
  }
  return `wrapper-argv0 gate: ${workflowPath} rejects ${finding.segment}`;
}

/**
 * The wrapper gate's positive anchor must be the executed-tests baseline's
 * source->count inventory. Return the source keys when `test_attribute_baseline` is a
 * plain object mapping nonblank source paths to non-negative integer counts, or null
 * when the anchor is absent or malformed (so the gate fails closed instead of counting
 * a string, an array, or an object of garbage as evidence).
 */
function validExecutedSourceInventory(testAttributeBaseline) {
  if (
    testAttributeBaseline === null
    || typeof testAttributeBaseline !== "object"
    || Array.isArray(testAttributeBaseline)
  ) {
    return null;
  }
  const sources = [];
  for (const [source, count] of Object.entries(testAttributeBaseline)) {
    if (typeof source !== "string" || source.trim() === "") return null;
    if (!Number.isInteger(count) || count < 0) return null;
    sources.push(source);
  }
  return sources;
}

function extractNamedWorkflowStep(text, name) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const matcher = new RegExp(`^(\\s*)-\\s+name:\\s*${escapeRegExp(name)}\\s*$`);
  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(matcher);
    if (!match) continue;
    const indent = match[1].length;
    const block = [lines[index]];
    for (
      let blockIndex = index + 1;
      blockIndex < lines.length;
      blockIndex += 1
    ) {
      const line = lines[blockIndex];
      if (line.trim() === "") {
        block.push(line);
        continue;
      }
      if (countLeadingSpaces(line) <= indent) break;
      block.push(line);
    }
    return block.join("\n");
  }
  return "";
}

function workflowHasActiveUse(text, regex) {
  return regex.test(stripHashComments(text));
}

function parseKustomizeImageEntries(text) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const entries = [];
  let current;
  for (const line of lines) {
    const nameMatch = line.match(/^\s*-\s*name:\s*(.+?)\s*$/);
    if (nameMatch) {
      current = {
        name: stripYamlScalar(nameMatch[1]),
        digest: undefined,
        newTag: undefined,
      };
      entries.push(current);
      continue;
    }
    if (!current) continue;
    const fieldMatch = line.match(/^\s*(digest|newTag):\s*(.+?)\s*$/);
    if (fieldMatch) current[fieldMatch[1]] = stripYamlScalar(fieldMatch[2]);
  }
  return entries;
}

function extractYamlSequenceItemNames(text, key, keyIndent = 2) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const keyMatcher = new RegExp(`^\\s{${keyIndent}}${escapeRegExp(key)}:\\s*$`);
  for (let index = 0; index < lines.length; index += 1) {
    if (!keyMatcher.test(lines[index])) continue;
    const names = [];
    for (
      let blockIndex = index + 1;
      blockIndex < lines.length;
      blockIndex += 1
    ) {
      const line = lines[blockIndex];
      if (line.trim() === "") continue;
      if (countLeadingSpaces(line) <= keyIndent) break;
      const nameMatch = line.match(/^\s*-\s*name:\s*(.+?)\s*$/);
      if (nameMatch) names.push(stripYamlScalar(nameMatch[1]));
    }
    return names.filter(Boolean);
  }
  return [];
}

function extractEnvVarBlock(text, envName) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const matcher = new RegExp(
    `^\\s*-\\s*name:\\s*${escapeRegExp(envName)}\\s*$`,
  );
  for (let index = 0; index < lines.length; index += 1) {
    if (!matcher.test(lines[index])) continue;
    const indent = countLeadingSpaces(lines[index]);
    const block = [lines[index]];
    for (
      let blockIndex = index + 1;
      blockIndex < lines.length;
      blockIndex += 1
    ) {
      const line = lines[blockIndex];
      if (/^---\s*$/.test(line)) break;
      if (line.trim() === "") {
        block.push(line);
        continue;
      }
      const lineIndent = countLeadingSpaces(line);
      if (lineIndent <= indent && /^\s*-\s*name\s*:/.test(line)) break;
      if (lineIndent < indent) break;
      block.push(line);
    }
    return block.join("\n");
  }
  return "";
}

function envVarRequiresSecretKeyRef(text, envName, secretName) {
  const block = extractEnvVarBlock(text, envName);
  if (!block || !/\bsecretKeyRef\s*:/.test(block)) return false;
  if (/^\s*optional\s*:\s*true\s*$/m.test(block)) return false;

  const quotedName = `["']?${escapeRegExp(secretName)}["']?`;
  const quotedKey = `["']?${escapeRegExp(envName)}["']?`;
  return (
    new RegExp(`\\bname:\\s*${quotedName}\\b`).test(block) &&
    new RegExp(`\\bkey:\\s*${quotedKey}\\b`).test(block)
  );
}

function braceDelta(line) {
  return (
    [...line].filter((char) => char === "{").length -
    [...line].filter((char) => char === "}").length
  );
}

function extractExecutableShellLines(text) {
  const lines = stripHashComments(text).split(/\r?\n/);
  const executable = [];
  let functionDepth = 0;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed === "") continue;
    if (functionDepth > 0) {
      functionDepth = Math.max(0, functionDepth + braceDelta(line));
      continue;
    }
    if (
      /^(?:function\s+)?[A-Za-z_][A-Za-z0-9_]*\s*(?:\(\s*\))?\s*\{/.test(
        trimmed,
      )
    ) {
      functionDepth = Math.max(0, braceDelta(line));
      continue;
    }
    executable.push(line);
  }
  return executable;
}

function combineShellContinuations(lines) {
  const logical = [];
  let current = "";
  for (const line of lines) {
    const trimmedRight = line.trimEnd();
    if (trimmedRight.endsWith("\\")) {
      current += `${trimmedRight.slice(0, -1)} `;
      continue;
    }
    current += trimmedRight;
    if (current.trim()) logical.push(current.trim());
    current = "";
  }
  if (current.trim()) logical.push(current.trim());
  return logical;
}

function parseShellArray(logicalLines, name) {
  const match = logicalLines
    .join("\n")
    .match(new RegExp(`^\\s*${escapeRegExp(name)}\\s*=\\s*\\(([^)]*)\\)`, "m"));
  if (!match) return [];
  return [...match[1].matchAll(/"([^"]+)"|'([^']+)'|(\S+)/g)]
    .map((valueMatch) => valueMatch[1] ?? valueMatch[2] ?? valueMatch[3])
    .filter(Boolean);
}

function findFailOpenKubectlPrerequisiteBlocks(logicalLines) {
  const failures = [];
  for (let index = 0; index < logicalLines.length; index += 1) {
    const line = logicalLines[index];
    if (!/^if\s+!/.test(line) || !/\b(?:kubectl|have\s+kubectl)\b/.test(line))
      continue;
    let thenText = line
      .split(/\bthen\b/)
      .slice(1)
      .join("then");
    for (
      let blockIndex = index + 1;
      blockIndex < logicalLines.length;
      blockIndex += 1
    ) {
      const blockLine = logicalLines[blockIndex];
      if (/^(?:else|fi)\b/.test(blockLine)) break;
      thenText += `\n${blockLine}`;
    }
    if (!/\b(?:exit|return)\s+[1-9]\d*\b/.test(thenText)) failures.push(line);
  }
  return failures;
}

function findDigestBumpOnlyVerificationClaims(logicalLines) {
  return logicalLines.filter(
    (line) =>
      /\b(?:digest-bump-only|bump-only|digest bump only|desired prod digests updated only)\b/i.test(
        line,
      ) &&
      /\bdeployed and verified\b|\bdeployment\b.*\bverified\b|\brollout\b.*\bverified\b|\bendpoint\b.*\bverified\b/i.test(
        line,
      ),
  );
}

export function evaluateProdOverlayImageChecks(readText) {
  const result = createResult();
  const path = "deploy/apps/console/overlays/prod/kustomization.yaml";
  const prodOverlay = requirePresentText(
    result,
    readText,
    path,
    "prod overlay kustomization",
  );
  const imageEntries = parseKustomizeImageEntries(prodOverlay);
  const requiredImages = ["console-app"];
  const pinnedRequiredImages = requiredImages.filter((imageName) =>
    imageEntries.some(
      (entry) =>
        entry.name === imageName &&
        /^sha256:[0-9a-f]{64}$/.test(entry.digest ?? ""),
    ),
  );
  requirement(
    result,
    pinnedRequiredImages.length === requiredImages.length,
    `prod overlay digest pins: ${pinnedRequiredImages.length} (${pinnedRequiredImages.join(", ")})`,
    `${path} must pin at least console-app by immutable sha256 digest (found ${pinnedRequiredImages.length}); do not deploy mutable tags`,
  );

  const mutableTags = imageEntries.filter(
    (entry) => entry.newTag !== undefined,
  );
  requirement(
    result,
    mutableTags.length === 0,
    "prod overlay has no mutable newTag values",
    `${path} must not use mutable newTag values (found ${mutableTags
      .map((entry) => `${entry.name}:${entry.newTag}`)
      .map(JSON.stringify)
      .join(", ")})`,
  );
  return result;
}

const PR473_ROLLBACK_FLOOR = "f6ff236b9770c79301a3d07da6afb56be1e27bbf";
const PR473_MANIFEST_PATH = "docs/release/PR-473-EXPAND-CONTRACT.gate.json";
const PR473_WRAPPER_ALIAS =
  "python3 scripts/check-pr473-migration-operational.py";
const PR473_TEST_ALIAS =
  "python3 scripts/check-pr473-migration-operational.test.py";
const PR473_WRAPPER_COMMAND = "npm run check:pr473-migration-operational";
const PR473_TOPOLOGY_COMMAND = [
  'APP_PASSWORD="$(openssl rand -hex 32)"',
  'RT_PASSWORD="$(openssl rand -hex 32)"',
  'LEAVE_COMMAND_PASSWORD="$(openssl rand -hex 32)"',
  'ONTOLOGY_COMMAND_PASSWORD="$(openssl rand -hex 32)"',
  'PLATFORM_FORCE_COMMAND_PASSWORD="$(openssl rand -hex 32)"',
  "docker run --rm --network host",
  '-v "$GITHUB_WORKSPACE/ops/postgres-reconcile-topology.sh:/usr/local/bin/postgres-reconcile-topology:ro"',
  "-e POSTGRES_HOST=127.0.0.1 -e POSTGRES_DB=console_ci",
  "-e POSTGRES_ADMIN_USER=postgres -e POSTGRES_ADMIN_PASSWORD=postgres",
  '-e CONSOLE_APP_POSTGRES_PASSWORD="$APP_PASSWORD"',
  '-e CONSOLE_RT_POSTGRES_PASSWORD="$RT_PASSWORD"',
  '-e CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD="$LEAVE_COMMAND_PASSWORD"',
  '-e CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD="$ONTOLOGY_COMMAND_PASSWORD"',
  '-e CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD="$PLATFORM_FORCE_COMMAND_PASSWORD"',
  "--entrypoint bash postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20",
  "/usr/local/bin/postgres-reconcile-topology",
  "docker run --rm --network host",
  "-e PGPASSWORD=postgres",
  "--entrypoint psql",
  "postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20",
  "-h 127.0.0.1 -U postgres -d postgres -v ON_ERROR_STOP=1",
  '-c "DROP DATABASE IF EXISTS console_apalis_contract WITH (FORCE)"',
  '-c "CREATE DATABASE console_apalis_contract OWNER console_app"',
  // Migration 0196 admits only `console_buck_admin` for applying migrations, so the
  // suites built on #[sqlx::test] cannot run as the `postgres` service account.
  // The password reaches psql through a mode-0600 file, never argv.
  'BUCK_ADMIN_PASSWORD="$(openssl rand -hex 32)"',
  "umask 077",
  'printf "CREATE ROLE console_buck_admin SUPERUSER LOGIN PASSWORD \'%s\';\\n"',
  '"$BUCK_ADMIN_PASSWORD" > "$RUNNER_TEMP/buck-admin.sql"',
  "docker run --rm --network host",
  "-e PGPASSWORD=postgres",
  '-v "$RUNNER_TEMP/buck-admin.sql:/buck-admin.sql:ro"',
  "--entrypoint psql",
  "postgres:18.4@sha256:4aabea78cf39b90e834caf3af7d602a18565f6fe2508705c8d01aa63245c2e20",
  "-h 127.0.0.1 -U postgres -d postgres -v ON_ERROR_STOP=1 -f /buck-admin.sql",
  'rm -f "$RUNNER_TEMP/buck-admin.sql"',
  'echo "::add-mask::$APP_PASSWORD"',
  'echo "::add-mask::$RT_PASSWORD"',
  'echo "::add-mask::$BUCK_ADMIN_PASSWORD"',
  "{",
  'echo "CONSOLE_BUCK_ADMIN_DATABASE_URL=postgres://console_buck_admin:${BUCK_ADMIN_PASSWORD}@localhost:5432/console_ci?options%5Bconsole.sqlx_test_bootstrap%5D=buck-sqlx-superuser-v1"',
  'echo "CONSOLE_APALIS_OWNER_DATABASE_URL=postgres://console_app:${APP_PASSWORD}@localhost:5432/console_apalis_contract"',
  'echo "CONSOLE_APALIS_RUNTIME_DATABASE_URL=postgres://console_rt:${RT_PASSWORD}@localhost:5432/console_apalis_contract"',
  'echo "CONSOLE_APALIS_ADMIN_DATABASE_URL=postgres://postgres:postgres@localhost:5432/console_apalis_contract"',
  '} >> "$GITHUB_ENV"',
].join(" ");
const PR473_DOCUMENTS = [
  {
    path: "docs/release/PR-473-ONTOLOGY-EXPAND-CONTRACT.md",
    title: "# PR 473 Ontology Expand Contract",
  },
  {
    path: "docs/release/PR-473-EMPLOYEE-IMPORT-EXPAND-CONTRACT.md",
    title: "# PR 473 Employee-Import Expand Contract",
  },
];
const PR473_DIRECTIVES = [
  "<!-- PR473-MIGRATION-GATE: release_phase=expand -->",
  "<!-- PR473-MIGRATION-GATE: deployment_authorized=false -->",
  "<!-- PR473-MIGRATION-GATE: command_only_claim_authorized=false -->",
  "<!-- PR473-MIGRATION-GATE: production_authority=production_cardinality,old_runtime_drain,rollback_floor_raise -->",
];
const PR473_TESTS = [
  [
    "ontology",
    "console-ontology-adapter-postgres",
    "key_revision_migration_upgrade",
    "backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs",
    "migration_0165_upgrades_legacy_sibling_versions_without_tenant_leakage",
  ],
  [
    "ontology",
    "console-ontology-adapter-postgres",
    "key_revision_migration_upgrade",
    "backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs",
    "migration_0165_keeps_exact_old_binary_writes_audited_and_cas_consistent",
  ],
  [
    "ontology",
    "console-ontology-adapter-postgres",
    "key_revision_migration_upgrade",
    "backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs",
    "migration_0165_rehearses_populated_expand_with_bounded_lock_and_statement_timeouts",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "migration_0166_rehearses_populated_expand_with_bounded_lock_and_statement_timeouts",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "exact_charge_create_accepts_resolved_and_review_required_shapes",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "exact_charge_create_atomically_rejects_mismatched_reason_and_evidence_shapes",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "immediate_f6ff_employee_import_remains_usable_after_0166",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "staged_f6ff_employee_import_apply_remains_atomic_after_0166",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "staged_f6ff_apply_rejects_missing_duplicate_or_forged_current_tx_audit",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "legacy_leave_mutations_require_exactly_one_same_transaction_audit",
  ],
  [
    "leave",
    "console-leave-adapter-postgres",
    "leave_migration_expand_contract",
    "backend/crates/leave/adapter-postgres/tests/leave_migration_expand_contract.rs",
    "staged_employee_import_rejects_payload_not_equal_to_immutable_ledger",
  ],
];

export function evaluateExpandContractReleaseChecks(readText) {
  const result = createResult();
  const manifestText = requirePresentText(
    result,
    readText,
    PR473_MANIFEST_PATH,
    "PR 473 typed migration gate manifest",
  );
  let manifest;
  try {
    manifest = JSON.parse(manifestText);
  } catch (error) {
    result.failures.push(
      `PR 473 typed migration gate manifest must be valid JSON (${error.message})`,
    );
    return result;
  }

  requirement(
    result,
    manifestText === `${JSON.stringify(manifest, null, 2)}\n`,
    "PR 473 typed migration gate manifest: canonical JSON",
    "PR 473 typed migration gate manifest must use canonical two-space JSON with one trailing newline",
  );
  const expectedKeys = [
    "schema_version",
    "pull_request",
    "rollback_floor",
    "release_phase",
    "deployment_authorized",
    "command_only_claim_authorized",
    "production_authority",
    "guarded_tests",
  ];
  requirement(
    result,
    manifest &&
      typeof manifest === "object" &&
      !Array.isArray(manifest) &&
      JSON.stringify(Object.keys(manifest)) === JSON.stringify(expectedKeys),
    "PR 473 typed migration gate manifest: exact schema keys",
    "PR 473 typed migration gate manifest must have only the canonical ordered schema keys",
  );
  for (const [label, actual, expected] of [
    ["schema version", manifest?.schema_version, 1],
    ["pull request", manifest?.pull_request, 473],
    ["rollback floor", manifest?.rollback_floor, PR473_ROLLBACK_FLOOR],
    ["expand phase", manifest?.release_phase, "expand"],
    ["nondeploy", manifest?.deployment_authorized, false],
    ["nonclaim", manifest?.command_only_claim_authorized, false],
  ]) {
    requirement(
      result,
      actual === expected && typeof actual === typeof expected,
      `PR 473 typed migration gate ${label}: ${JSON.stringify(expected)}`,
      `PR 473 typed migration gate ${label} must be exactly ${JSON.stringify(expected)}`,
    );
  }
  const expectedProductionAuthority = {
    production_cardinality: false,
    old_runtime_drain: false,
    rollback_floor_raise: false,
  };
  requirement(
    result,
    JSON.stringify(manifest?.production_authority) ===
      JSON.stringify(expectedProductionAuthority),
    "PR 473 production-authority fields: all false",
    "PR 473 production-authority fields must be exactly production_cardinality, old_runtime_drain, and rollback_floor_raise set to false",
  );

  const tests = Array.isArray(manifest?.guarded_tests)
    ? manifest.guarded_tests
    : [];
  const tuples = tests.map((test) => [
    test?.domain,
    test?.package,
    test?.target,
    test?.source,
    test?.name,
  ]);
  const uniqueTuples = new Set(tuples.map((tuple) => JSON.stringify(tuple)));
  const uniqueNames = new Set(tuples.map((tuple) => tuple[4]));
  requirement(
    result,
    tests.length === 11 && uniqueTuples.size === 11 && uniqueNames.size === 11,
    "PR 473 guarded tests: 11 unique tuples and names",
    "PR 473 guarded tests must contain 11 unique tuples with 11 unique names",
  );
  requirement(
    result,
    JSON.stringify(tuples) === JSON.stringify(PR473_TESTS),
    "PR 473 guarded tests: exact expected 3 ontology and 8 leave tuples",
    "PR 473 guarded tests must equal the exact expected 3 ontology and 8 leave tuples in canonical order",
  );

  for (const document of PR473_DOCUMENTS) {
    const text = requirePresentText(
      result,
      readText,
      document.path,
      `PR 473 gate document ${document.path}`,
    );
    const canonicalPrefix = `${document.title}\n\n${PR473_DIRECTIVES.join("\n")}\n\n`;
    requirement(
      result,
      text.startsWith(canonicalPrefix),
      `PR 473 document directives form canonical block: ${document.path}`,
      `PR 473 document directives must form the canonical block immediately after the title in ${document.path}`,
    );
    for (const directive of PR473_DIRECTIVES) {
      const count = text.split(directive).length - 1;
      requirement(
        result,
        count === 1,
        `PR 473 document directive occurs once: ${document.path} ${directive}`,
        `PR 473 document directive must occur exactly once in ${document.path}: ${directive} (found ${count})`,
      );
    }
  }

  let packageJson;
  try {
    packageJson = JSON.parse(readText("package.json"));
  } catch (error) {
    result.failures.push(
      `PR 473 package alias requires valid package.json (${error.message})`,
    );
  }
  requirement(
    result,
    packageJson?.scripts?.["check:pr473-migration-operational"] ===
      PR473_WRAPPER_ALIAS,
    "PR 473 package alias: exact wrapper command",
    `PR 473 package alias must be exactly ${JSON.stringify(PR473_WRAPPER_ALIAS)}`,
  );
  requirement(
    result,
    packageJson?.scripts?.["test:pr473-migration-operational"] ===
      PR473_TEST_ALIAS,
    "PR 473 unit-test package alias: exact test command",
    `PR 473 unit-test package alias must be exactly ${JSON.stringify(PR473_TEST_ALIAS)}`,
  );

  const ciText = readText(".github/workflows/ci.yml");
  const activeCi = stripHashComments(ciText);
  // The rehearsal moved out of `backend` into its own job 2026-08-18 (it was 445s
  // of a 1176s job, and `backend` was the critical path). The invariant this gate
  // enforces is unchanged -- the wrapper still runs exactly once, still after the
  // topology reconcile, still from the repository root -- so it now reads the job
  // that owns the step rather than the job that used to.
  const backendJob = extractYamlMappingBlock(activeCi, "migration-expand-contract", 2);
  const topologyStep = extractNamedWorkflowStep(
    backendJob,
    "Reconcile portable PostgreSQL role topology",
  );
  const wrapperStep = extractNamedWorkflowStep(
    backendJob,
    "Expand/contract migration rehearsal (0165, 0166)",
  );
  const topologyIndex = backendJob.indexOf(topologyStep);
  const wrapperIndex = backendJob.indexOf(wrapperStep);
  const invocationCount = extractGithubWorkflowRunBlocks(activeCi).filter(
    (block) => block === PR473_WRAPPER_COMMAND,
  ).length;
  const topologyRuns = extractGithubWorkflowRunBlocks(topologyStep);
  const topologyCommand = topologyRuns[0] ?? "";
  requirement(
    result,
    invocationCount === 1,
    "PR 473 CI wrapper invocation: exactly one active command",
    `PR 473 CI must contain exactly one active ${PR473_WRAPPER_COMMAND} invocation (found ${invocationCount})`,
  );
  requirement(
    result,
    topologyStep !== "" &&
      wrapperStep !== "" &&
      topologyIndex >= 0 &&
      wrapperIndex > topologyIndex,
    "PR 473 CI wrapper runs after PostgreSQL role topology",
    "migration-expand-contract job must contain the topology and wrapper steps in that order",
  );
  requirement(
    result,
    topologyRuns.length === 1 && topologyCommand === PR473_TOPOLOGY_COMMAND,
    "PR 473 backend topology step invokes the exact pinned reconcile and Apalis database provisioning commands",
    "PR 473 backend topology step must invoke the exact reconcile command and Apalis database provisioning command, credential setup, URL exports, and masking through the pinned PostgreSQL image",
  );
  requirement(
    result,
    JSON.stringify(extractGithubWorkflowRunBlocks(wrapperStep)) ===
      JSON.stringify([PR473_WRAPPER_COMMAND]),
    "PR 473 named CI wrapper step runs the exact command",
    `PR 473 named CI wrapper step must run exactly ${PR473_WRAPPER_COMMAND}`,
  );
  requirement(
    result,
    /^\s*working-directory:\s*\.\s*$/m.test(wrapperStep),
    "PR 473 CI wrapper runs from repository root",
    "PR 473 CI wrapper step must set working-directory to the repository root",
  );

  return result;
}

const workflowDirectory = ".github/workflows";
// Local composite actions live in-repo and are already covered by the commit
// under review; every remote reference must name an immutable commit SHA.
const localActionReference = /^\.{1,2}\//;
const pinnedActionReference = /^[^\s@]+@[0-9a-f]{40}$/;

export function listWorkflowFiles() {
  const absolute = resolve(root, workflowDirectory);
  if (!existsSync(absolute)) return [];
  return readdirSync(absolute)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .sort()
    .map((name) => `${workflowDirectory}/${name}`);
}

function workflowUsesReferences(text) {
  const references = [];
  for (const rawLine of text.split(/\r?\n/)) {
    const match = /^(?:-\s+)?uses:\s*(\S.*)$/.exec(rawLine.trim());
    if (match) references.push(stripYamlScalar(match[1]));
  }
  return references;
}

export function evaluateActionPinChecks(
  readText,
  workflowPaths = listWorkflowFiles(),
) {
  const result = createResult();
  requirement(
    result,
    workflowPaths.length > 0,
    `action pins: ${workflowPaths.length} workflow files enumerated from ${workflowDirectory}`,
    `action pins: no workflow files found under ${workflowDirectory}; the pin gate would assert nothing`,
  );

  let pinned = 0;
  let local = 0;
  for (const path of workflowPaths) {
    const text = readText(path);
    if (text.trim().length === 0) {
      result.failures.push(
        `action pins: ${path} is missing or empty; its action references cannot be proven pinned`,
      );
      continue;
    }
    for (const reference of workflowUsesReferences(text)) {
      if (localActionReference.test(reference)) {
        local += 1;
      } else if (pinnedActionReference.test(reference)) {
        pinned += 1;
      } else {
        result.failures.push(
          `action pins: ${path} uses ${JSON.stringify(reference)}, which is not pinned to a full 40-hex commit SHA`,
        );
      }
    }
  }

  requirement(
    result,
    pinned > 0,
    `action pins: ${pinned} remote action references SHA-pinned across ${workflowPaths.length} workflow files (${local} local composite references exempt)`,
    "action pins: no SHA-pinned remote action references were found; the pin gate is asserting nothing",
  );
  return result;
}

export function evaluateWorkflowHardeningChecks(readText) {
  const result = createResult();
  const productionHardeningTestCommand =
    "npm run test:pr473-migration-operational && python3 scripts/check-production-promotion-authority.test.py && node --test scripts/check-production-authority-blocked.test.mjs scripts/check-production-hardening.test.mjs scripts/check-image-release-workflow.test.mjs";
  const ciPath = ".github/workflows/ci.yml";
  const securityPath = ".github/workflows/security.yml";
  const imageReleasePath = ".github/workflows/image-release.yml";
  const promotionAuthorityPath =
    "scripts/check-production-promotion-authority.py";
  const productionAuthorizationPath =
    "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json";
  const productionEvidencePath =
    "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json";
  const productionPromotionContractPath =
    "docs/release/PR-473-PRODUCTION-PROMOTION.md";
  const ciWorkflow = requirePresentText(
    result,
    readText,
    ciPath,
    "CI workflow",
  );
  const securityWorkflow = requirePresentText(
    result,
    readText,
    securityPath,
    "Security workflow",
  );
  const imageReleaseWorkflow = requirePresentText(
    result,
    readText,
    imageReleasePath,
    "image-release workflow",
  );
  const promotionAuthority = requirePresentText(
    result,
    readText,
    promotionAuthorityPath,
    "production promotion authority verifier",
  );
  const productionAuthorizationText = requirePresentText(
    result,
    readText,
    productionAuthorizationPath,
    "production promotion authorization record",
  );
  const productionEvidenceText = requirePresentText(
    result,
    readText,
    productionEvidencePath,
    "production cardinality evidence template",
  );
  const productionPromotionContract = requirePresentText(
    result,
    readText,
    productionPromotionContractPath,
    "production promotion contract",
  );
  let productionAuthorization;
  try {
    productionAuthorization = JSON.parse(productionAuthorizationText);
  } catch (error) {
    result.failures.push(
      `production promotion authorization record must be valid JSON (${error.message})`,
    );
  }
  const expectedAuthorizationKeys = [
    "schema_version",
    "pull_request",
    "target",
    "release_phase",
    "rollback_floor",
    "desired_state_authority_cutover",
    "deployment_authorized",
    "command_only",
    "production_cardinality_evidence",
    "contract_authorities",
  ];
  const cardinalityEvidence =
    productionAuthorization?.production_cardinality_evidence;
  const cardinalityEvidenceText =
    typeof cardinalityEvidence?.path === "string"
      ? readText(cardinalityEvidence.path)
      : "";
  const cardinalityEvidenceHash = createHash("sha256")
    .update(cardinalityEvidenceText)
    .digest("hex");
  let productionEvidence;
  try {
    productionEvidence = JSON.parse(productionEvidenceText);
  } catch (error) {
    result.failures.push(
      `production cardinality evidence template must be valid JSON (${error.message})`,
    );
  }
  const expectedEvidenceKeys = [
    "schema_version",
    "target",
    "release_phase",
    "candidate_source_sha",
    "observed_running_revision",
    "observed_database_topology",
    "capacity_headroom",
    "backup_restore_proof",
    "evidence_author",
    "independent_reviewer",
    "charter",
    "observed_at",
    "prepared_at",
    "reviewed_at",
  ];
  requirement(
    result,
    productionAuthorizationText ===
      `${JSON.stringify(productionAuthorization, null, 2)}\n` &&
      JSON.stringify(Object.keys(productionAuthorization ?? {})) ===
        JSON.stringify(expectedAuthorizationKeys) &&
      productionAuthorization?.schema_version === 2 &&
      productionAuthorization?.pull_request === 473 &&
      productionAuthorization?.target === "production" &&
      productionAuthorization?.release_phase === "expand" &&
      productionAuthorization?.rollback_floor === PR473_ROLLBACK_FLOOR &&
      productionAuthorization?.desired_state_authority_cutover === false &&
      productionAuthorization?.deployment_authorized === false &&
      productionAuthorization?.command_only === false &&
      JSON.stringify(Object.keys(cardinalityEvidence ?? {})) ===
        JSON.stringify(["path", "sha256", "verified"]) &&
      cardinalityEvidence?.path === productionEvidencePath &&
      /^[0-9a-f]{64}$/.test(cardinalityEvidence?.sha256 ?? "") &&
      cardinalityEvidence?.sha256 === cardinalityEvidenceHash &&
      cardinalityEvidence?.verified === false &&
      productionEvidenceText ===
        `${JSON.stringify(productionEvidence, null, 2)}\n` &&
      JSON.stringify(Object.keys(productionEvidence ?? {})) ===
        JSON.stringify(expectedEvidenceKeys) &&
      productionEvidence?.candidate_source_sha === "0".repeat(40) &&
      productionEvidence?.independent_reviewer?.team_id === 0 &&
      productionEvidence?.evidence_author?.github_login ===
        "TEMPLATE_NOT_EVIDENCE" &&
      JSON.stringify(productionAuthorization?.contract_authorities) ===
        JSON.stringify({
          old_runtime_drain: false,
          rollback_floor_raise: false,
        }),
    "production promotion authorization: canonical schema-v2 false record",
    "production promotion authorization must be the canonical schema-v2 false-by-default production expand record",
  );
  requirement(
    result,
    /mutable `main`/.test(productionPromotionContract) &&
      /`desired_state_authority_cutover` field is immutable `false`/.test(
        productionPromotionContract,
      ) &&
      /activation remains \*\*BLOCKED\*\*/.test(productionPromotionContract) &&
      /separate, higher-authority ADR/.test(productionPromotionContract) &&
      /self-asserted strings whose provenance is not authenticated/.test(
        productionPromotionContract,
      ) &&
      /administrator bypass posture/.test(productionPromotionContract) &&
      /does \*\*not\*\* make mutable `main` a safe production desired-state authority/.test(
        productionPromotionContract,
      ),
    "production promotion contract: mutable-main activation block is explicit",
    "production promotion contract must state that mutable-main activation remains blocked pending a separate accepted desired-state-authority ADR/cutover",
  );
  let packageJson;
  try {
    packageJson = JSON.parse(readText("package.json"));
  } catch (error) {
    result.failures.push(
      `production-hardening test command requires valid package.json (${error.message})`,
    );
  }
  requirement(
    result,
    packageJson?.scripts?.["test:production-hardening"] ===
      productionHardeningTestCommand,
    "production-hardening tests execute the production promotion authority suite",
    "test:production-hardening must execute the production promotion authority test suite in the canonical fail-closed command",
  );
  requirement(
    result,
    packageJson?.scripts?.["check:production-authority-blocked"] ===
      "node scripts/check-production-authority-blocked.mjs" &&
      readText("scripts/check-production-authority-blocked.mjs").trim().length >
        0,
    "production authority blocked evaluator: explicit-SHA CLI wiring present",
    "production authority blocked evaluator and exact package CLI wiring must be present",
  );
  requirement(
    result,
    workflowHasRun(ciWorkflow, [/\bnpm\s+run\s+check:production-hardening\b/]),
    "CI runs production-hardening contract as an active step",
    "CI must run npm run check:production-hardening as an active step",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /\bnpm\s+run\s+check:production-hardening\b/,
    ]),
    "Security workflow runs production-hardening contract as an active step",
    "Security workflow must run npm run check:production-hardening as an active step",
  );

  const activeImageRelease = stripHashComments(imageReleaseWorkflow);
  const workflowRunTrigger = extractYamlMappingBlock(
    activeImageRelease,
    "workflow_run",
    2,
  );
  const ciAdmissionJob = extractYamlMappingBlock(
    activeImageRelease,
    "ci-admission",
    2,
  );
  const buildJob = extractYamlMappingBlock(activeImageRelease, "build", 2);
  const mergeJob = extractYamlMappingBlock(activeImageRelease, "merge", 2);
  const releaseProbeForAdmission = extractYamlMappingBlock(
    activeImageRelease,
    "release-probe",
    2,
  );
  requirement(
    result,
    /workflows:\s*\["CI"\]/.test(workflowRunTrigger) &&
      /types:\s*\[completed\]/.test(workflowRunTrigger) &&
      /branches:\s*\[dev\]/.test(workflowRunTrigger) &&
      !/^  push:/m.test(activeImageRelease) &&
      ciAdmissionJob !== "" &&
      /TRIGGER_RUN_ID:\s*\$\{\{\s*github\.event\.workflow_run\.id\s*\}\}/.test(ciAdmissionJob) &&
      /TRIGGER_WORKFLOW_HEAD_SHA:\s*\$\{\{\s*github\.event\.workflow_run\.head_sha\s*\}\}/.test(ciAdmissionJob) &&
      /CI_WORKFLOW_ID="296023727"/.test(ciAdmissionJob) &&
      /SECURITY_WORKFLOW_ID="296023731"/.test(ciAdmissionJob) &&
      /contents:\s*read/.test(ciAdmissionJob) &&
      /actions:\s*read/.test(ciAdmissionJob) &&
      /eligible:\s*\$\{\{\s*steps\.admit\.outputs\.eligible\s*\}\}/.test(ciAdmissionJob) &&
      /eligible=false/.test(ciAdmissionJob) &&
      /eligible=true/.test(ciAdmissionJob) &&
      /\.immutable == true/.test(ciAdmissionJob) &&
      /releases\/tags\/\$\{release_tag\}/.test(ciAdmissionJob) &&
      /git\/ref\/tags\/\$\{release_tag\}/.test(ciAdmissionJob) &&
      /Required \/ CI/.test(ciAdmissionJob) &&
      /Required \/ Security/.test(ciAdmissionJob) &&
      /actions\/workflows\/\$\{workflow_id\}\/runs\?event=push&branch=dev&head_sha=\$\{candidate_sha\}&per_page=100/.test(ciAdmissionJob) &&
      /repos\/\$\{REPO\}\/git\/ref\/heads\/dev/.test(ciAdmissionJob) &&
      /release manifest version is unchanged/.test(ciAdmissionJob) &&
      /candidate stopped being current dev before publication authorization/.test(ciAdmissionJob) &&
      /RUN_ATTEMPT" == "1/.test(ciAdmissionJob) &&
      /timeout 20s gh api/.test(ciAdmissionJob) &&
      /ADMISSION_MAX_POLLS:\s*"48"/.test(ciAdmissionJob) &&
      /release_sha=\$candidate_sha/.test(ciAdmissionJob) &&
      /release_tag=\$release_tag/.test(ciAdmissionJob) &&
      !/actions\/checkout|actions\/cache|download-artifact|gh\s+run\s+list/.test(ciAdmissionJob) &&
      /needs:\s*ci-admission/.test(buildJob) &&
      /if:\s*needs\.ci-admission\.outputs\.eligible == 'true'/.test(buildJob) &&
      /needs:\s*(?:build|\[[^\]]*\bbuild\b[^\]]*\])/.test(mergeJob) &&
      /needs:\s*(?:merge|\[[^\]]*\bmerge\b[^\]]*\])/.test(releaseProbeForAdmission) &&
      (activeImageRelease.match(/needs\.ci-admission\.outputs\.release_sha/g) ?? []).length >= 5,
    "image-release admission: completed CI wake-up, immutable release classifier, and exact CI/Security proof before publication",
    "image-release must use completed CI only as a wake-up, classify ordinary candidates as ineligible, and require one immutable exact-SHA release plus exact CI/Security before any build, scan, sign, promote, or publication",
  );
  const workflowDispatch = extractYamlMappingBlock(
    activeImageRelease,
    "workflow_dispatch",
    2,
  );
  const candidateShaInput = extractYamlMappingBlock(
    workflowDispatch,
    "candidate_sha",
    6,
  );
  const productionPromotionInput = extractYamlMappingBlock(
    workflowDispatch,
    "promote_production",
    6,
  );
  requirement(
    result,
      extractYamlScalar(candidateShaInput, "required") === "true" &&
      extractYamlScalar(candidateShaInput, "type") === "string" &&
      /DISPATCH_CANDIDATE_SHA:\s*\$\{\{\s*inputs\.candidate_sha\s*\}\}/.test(ciAdmissionJob) &&
      /\[\[ "\$DISPATCH_REF" == "refs\/heads\/dev" \]\]/.test(ciAdmissionJob) &&
      /\[\[ "\$RUN_ATTEMPT" == "1" \]\]/.test(ciAdmissionJob) &&
      /\[\[ "\$candidate_sha" =~ \^\[0-9a-f\]\{40\}\$ \]\]/.test(ciAdmissionJob),
    "image-release recovery: required exact-SHA first-attempt dev dispatch",
    "image-release workflow_dispatch recovery must require a lowercase 40-character candidate_sha and reject non-dev or rerun attempts",
  );
  requirement(
    result,
    extractYamlScalar(productionPromotionInput, "required") === "true" &&
      extractYamlScalar(productionPromotionInput, "default") === "false" &&
      extractYamlScalar(productionPromotionInput, "type") === "boolean",
    "image-release production promotion: explicit required false-by-default boolean dispatch input",
    "image-release workflow_dispatch must declare promote_production as a required false-by-default boolean",
  );

  const bumpDigestJob = extractYamlMappingBlock(
    activeImageRelease,
    "bump-digests",
    2,
  );
  const promotionPreflightJob = extractYamlMappingBlock(
    activeImageRelease,
    "production-promotion-preflight",
    2,
  );
  const bumpDigestSteps = bumpDigestJob.indexOf("\n    steps:");
  const bumpDigestJobHeader =
    bumpDigestSteps >= 0
      ? bumpDigestJob.slice(0, bumpDigestSteps)
      : bumpDigestJob;
  requirement(
    result,
    /github\.event_name\s*==\s*["']workflow_dispatch["']/.test(
      bumpDigestJobHeader,
    ) &&
      /github\.ref\s*==\s*["']refs\/heads\/dev["']/.test(
        bumpDigestJobHeader,
      ) &&
      /inputs\.promote_production\s*==\s*true/.test(bumpDigestJobHeader) &&
      /github\.run_attempt\s*==\s*1/.test(bumpDigestJobHeader) &&
      !/github\.event_name\s*==\s*["']push["']/.test(bumpDigestJobHeader),
    "image-release production promotion: manual dispatch on dev with explicit true input",
    "image-release bump-digests must run only for an explicit workflow_dispatch on refs/heads/dev with inputs.promote_production == true; push events must never mutate production",
  );
  requirement(
    result,
    promotionPreflightJob !== "" &&
      !/^\s{4}environment:/m.test(promotionPreflightJob) &&
      /^\s{6}contents:\s*read\s*$/m.test(promotionPreflightJob) &&
      /persist-credentials:\s*false/.test(promotionPreflightJob) &&
      /python3 scripts\/check-production-promotion-authority\.py initial/.test(
        promotionPreflightJob,
      ) &&
      /github\.run_attempt\s*==\s*1/.test(promotionPreflightJob) &&
      /production-promotion-preflight/.test(bumpDigestJobHeader),
    "image-release production promotion: unprotected read-only authorization preflight",
    "image-release must reject invalid authorization in an unprotected read-only preflight before the protected mutation job",
  );
  requirement(
    result,
    /^\s{4}environment:\s*["']?production["']?\s*$/m.test(bumpDigestJobHeader),
    "image-release production promotion: mutation job bound to production environment",
    "image-release must bind the mutation job to the production environment",
  );
  const exactCheckoutIndex = bumpDigestJob.search(
    /uses:\s*actions\/checkout@[0-9a-f]{40}[\s\S]*?ref:\s*\$\{\{\s*needs\.ci-admission\.outputs\.release_sha\s*\}\}/,
  );
  const initialAuthorityIndex = bumpDigestJob.indexOf(
    "python3 scripts/check-production-promotion-authority.py initial",
  );
  const mutationIndex = bumpDigestJob.indexOf("scripts/bump-prod-digests.sh");
  const resetAuthorityIndex = bumpDigestJob.indexOf(
    "python3 scripts/check-production-promotion-authority.py reset",
  );
  const commitIndex = bumpDigestJob.indexOf("git commit");
  const prePushAuthorityIndex = bumpDigestJob.indexOf(
    "python3 scripts/check-production-promotion-authority.py pre-push",
  );
  const pushMatches =
    bumpDigestJob.match(/\bgit\s+push\s+origin\s+["']?HEAD:dev["']?/g) ?? [];
  const pushIndex = bumpDigestJob.search(
    /\bgit\s+push\s+origin\s+["']?HEAD:dev["']?/,
  );
  requirement(
    result,
    exactCheckoutIndex >= 0 &&
      initialAuthorityIndex > exactCheckoutIndex &&
      mutationIndex > initialAuthorityIndex &&
      resetAuthorityIndex > mutationIndex &&
      commitIndex > resetAuthorityIndex,
    "image-release production promotion: exact-SHA authorization and consumption",
    "image-release bump-digests must re-check the admitted exact SHA, bump digests, reset one-shot authorization, and then commit",
  );
  requirement(
    result,
    commitIndex >= 0 &&
      prePushAuthorityIndex > commitIndex &&
      pushIndex > prePushAuthorityIndex &&
      pushMatches.length === 1,
    "image-release production promotion: immutable-dev recheck precedes one push",
    "image-release bump-digests must re-check origin/dev after commit and immediately precede exactly one normal HEAD:dev push",
  );
  requirement(
    result,
    !/\bgit\s+(?:pull|rebase)\b|\brebase\b|\bretr(?:y|ies|ied|ying)\b|\bfor\s+attempt\b/i.test(
      bumpDigestJob,
    ),
    "image-release production promotion: no rebase or retry path",
    "image-release bump-digests must not pull, rebase, retry, or loop after a racing dev push",
  );
  requirement(
    result,
    [
      /AUTHORIZATION_PATH\s*=\s*["']docs\/release\/PR-473-PRODUCTION-PROMOTION\.authorization\.json["']/,
      /CANONICAL_EVIDENCE_PATH\s*=\s*["']docs\/release\/PR-473-PRODUCTION-CARDINALITY\.evidence\.json["']/,
      /schema_version/,
      /authorized:\s*bool/,
      /pull_request/,
      /production_cardinality/,
      /old_runtime_drain/,
      /rollback_floor_raise/,
      /desired_state_authority_cutover/,
      /activation requires a separate accepted higher-authority ADR\/cutover/,
      /hashlib\.sha256/,
      /PurePosixPath/,
      /canonical_false/,
      /verify_evidence_schema/,
      /candidate_source_sha/,
      /independent_reviewer/,
      /team_id/,
      /commit_parent/,
      /diff-tree/,
      /git\(["']fetch["'],\s*["']--no-tags["'],\s*["']origin["'],\s*["']\+refs\/heads\/dev:refs\/remotes\/origin\/dev["']\)/,
      /git\(["']rev-parse["'],\s*["']HEAD["']\)/,
      /["']git["'],\s*["']show["'],\s*f["']\{expected_sha\}:\{path\}["']/,
      /git\(["']status["'],\s*["']--porcelain["'],\s*["']--untracked-files=no["']\)/,
      /keys are not exact/,
      /origin\/dev advanced after authorization/,
    ].every((pattern) => pattern.test(promotionAuthority)),
    "production promotion verifier: immutable typed authorization state machine",
    "production promotion authority verifier must fail closed on immutable git-show inputs, exact schema-v2 types, evidence hash, false-to-true authorization, reset commit shape, and origin/dev races",
  );
  requirement(
    result,
    workflowHasRun(bumpDigestJob, [
      /\bset\s+-euo\s+pipefail\b/,
      /check-production-promotion-authority\.py\s+reviewer-context/,
      /\bgh\s+api\b.*repos\/\$\{REPO\}\/environments\/production/,
      /prevent_self_review\s*==\s*true/,
      /\brequired_reviewers\b/,
      /reviewers\s*\|\s*length\)\s*==\s*1/,
      /reviewers\[0\]\.type\s*==\s*["']Team["']/,
      /reviewers\[0\]\.reviewer\.id\s*==\s*\$team_id/,
      /\$\{DISPATCHER,,\}/,
      /\$\{TRIGGERING_ACTOR,,\}/,
      /\$\{RUN_ATTEMPT\}/,
      /production promotion rejects workflow reruns/,
      /evidence_author_login/,
      /independent_reviewer_login/,
      /dispatcher must be distinct from the evidence author and independent evidence reviewer/,
    ]) &&
      /DISPATCHER:\s*\$\{\{\s*github\.actor\s*\}\}/.test(bumpDigestJob) &&
      /TRIGGERING_ACTOR:\s*\$\{\{\s*github\.triggering_actor\s*\}\}/.test(
        bumpDigestJob,
      ) &&
      /RUN_ATTEMPT:\s*\$\{\{\s*github\.run_attempt\s*\}\}/.test(
        bumpDigestJob,
      ) &&
      /^\s{6}actions:\s*read\s*$/m.test(bumpDigestJobHeader),
    "image-release production promotion: exact independent Team reviewer contract",
    "image-release production promotion must grant actions: read and fail closed unless the dispatcher is independent, prevent_self_review is true, and the exact immutable evidence Team ID is required",
  );

  const releaseProbe = extractYamlMappingBlock(
    activeImageRelease,
    "release-probe",
    2,
  );
  const releaseProbeCheckout = releaseProbe.search(
    /^\s*uses:\s*actions\/checkout@[0-9a-f]{40}\s*$/m,
  );
  const releaseProbeTopologyUse = releaseProbe.indexOf(
    "ops/postgres-reconcile-topology.sh",
  );
  const releaseProbeSteps = releaseProbe.indexOf("\n    steps:");
  const releaseProbeJobHeader =
    releaseProbeSteps >= 0
      ? releaseProbe.slice(0, releaseProbeSteps)
      : releaseProbe;
  requirement(
    result,
    /^\s{6}contents:\s*read\s*$/m.test(releaseProbeJobHeader),
    "image-release release-probe: job-level contents read permission",
    "image-release release-probe permissions must explicitly grant contents: read for its checkout",
  );
  requirement(
    result,
    releaseProbeCheckout >= 0 &&
      releaseProbeTopologyUse > releaseProbeCheckout &&
      /persist-credentials:\s*false/.test(
        releaseProbe.slice(releaseProbeCheckout, releaseProbeTopologyUse),
      ),
    "image-release release-probe: pinned credential-free checkout precedes local topology script use",
    "image-release release-probe must perform a SHA-pinned actions/checkout with persist-credentials: false before using ops/postgres-reconcile-topology.sh",
  );
  // Static ratchet for console-soe / console-bqg: the release-probe must invoke
  // canonical-enforce.sh AFTER image migrate and BEFORE boot. Without this
  // needle, deleting the step leaves check-production-hardening green while the
  // census silently returns to the pre-migration structural no-op.
  const releaseProbeMigrate = extractNamedWorkflowStep(
    releaseProbe,
    "Run migrations against the probe database as console_app",
  );
  const releaseProbeCanonicalEnforce = extractNamedWorkflowStep(
    releaseProbe,
    "Enforce canonical writer ownership on a migrated probe database",
  );
  const releaseProbeBoot = extractNamedWorkflowStep(
    releaseProbe,
    "Boot the release image (the real published image — no dev-auth flag exists to pass)",
  );
  const releaseProbeMigrateIndex = releaseProbe.indexOf(releaseProbeMigrate);
  const releaseProbeCanonicalEnforceIndex = releaseProbe.indexOf(
    releaseProbeCanonicalEnforce,
  );
  const releaseProbeBootIndex = releaseProbe.indexOf(releaseProbeBoot);
  requirement(
    result,
    releaseProbeMigrate !== "" &&
      releaseProbeCanonicalEnforce !== "" &&
      releaseProbeBoot !== "" &&
      releaseProbeMigrateIndex >= 0 &&
      releaseProbeCanonicalEnforceIndex > releaseProbeMigrateIndex &&
      releaseProbeBootIndex > releaseProbeCanonicalEnforceIndex &&
      /backend\/ci\/gates\/writer-ownership\/canonical-enforce\.sh/.test(
        releaseProbeCanonicalEnforce,
      ) &&
      /\bcanonical_probe_release\b/.test(releaseProbeCanonicalEnforce),
    "image-release release-probe: post-migration canonical-enforce",
    "image-release release-probe must run backend/ci/gates/writer-ownership/canonical-enforce.sh on a migrated probe database after migrate and before boot",
  );
  requirement(
    result,
    workflowHasRun(imageReleaseWorkflow, [
      /\btrivy\s+image\b/,
      /--exit-code\s+1\b/,
      /--severity\s+HIGH,CRITICAL\b/,
    ]),
    "image-release portable gate: active Trivy image scan fails HIGH/CRITICAL",
    "image-release must actively run a Trivy image scan with --exit-code 1 and HIGH,CRITICAL severity",
  );
  requirement(
    result,
    workflowHasRun(imageReleaseWorkflow, [/\bcosign\s+sign\s+--yes\b/]),
    "image-release portable gate: active cosign signing",
    "image-release must actively cosign sign the immutable image digest",
  );
  requirement(
    result,
    workflowHasActiveUse(
      imageReleaseWorkflow,
      /^\s*uses:\s*sigstore\/cosign-installer@[0-9a-f]{40}\s*$/m,
    ),
    "image-release portable gate: cosign installer pinned to upstream sigstore commit",
    "image-release must install cosign from sigstore/cosign-installer pinned to a full 40-hex commit SHA, not a fork or a floating tag",
  );
  requirement(
    result,
    workflowHasActiveUse(
      imageReleaseWorkflow,
      /^\s*uses:\s*actions\/attest-build-provenance@[0-9a-f]{40}\s*$/m,
    ),
    "image-release portable gate: provenance attestation pinned to upstream commit",
    "image-release must use actions/attest-build-provenance pinned to a full 40-hex commit SHA, not a fork or a floating tag",
  );
  requirement(
    result,
    workflowHasActiveUse(
      imageReleaseWorkflow,
      /^\s*uses:\s*actions\/attest-build-provenance@/m,
    ),
    "image-release portable gate: active provenance attestation",
    "image-release must actively use actions/attest-build-provenance",
  );
  requirement(
    result,
    workflowHasRun(imageReleaseWorkflow, [
      /\bscripts\/bump-prod-digests\.sh\b/,
    ]),
    "image-release portable gate: active bump-prod-digests",
    "image-release must actively run scripts/bump-prod-digests.sh",
  );

  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /\btrivy\s+fs\b/,
      /--scanners\s+vuln,secret\b/,
      /--exit-code\s+1\b/,
    ]),
    "security workflow portable gate: active Trivy filesystem vuln/secret scan",
    "security workflow must actively run trivy fs --scanners vuln,secret with --exit-code 1",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /\btrivy\s+config\b/,
      /--severity\s+HIGH,CRITICAL\b/,
      /--exit-code\s+1\b/,
    ]),
    "security workflow portable gate: active Trivy config scan",
    "security workflow must actively run trivy config with HIGH,CRITICAL and --exit-code 1",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [/\bcargo\s+audit\b/]),
    "security workflow portable gate: active cargo audit",
    "security workflow must actively run cargo audit",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /\bcargo\s+deny\b/,
      /--manifest-path\s+backend\/Cargo\.toml\b/,
      /\bcheck\b/,
    ]),
    "security workflow portable gate: active cargo deny",
    "security workflow must actively run cargo deny --manifest-path backend/Cargo.toml check",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [/\bnpm\s+audit\s+--audit-level=high\b/]),
    "security workflow portable gate: active npm audit",
    "security workflow must actively run npm audit --audit-level=high",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /npm audit --omit=dev --audit-level=high --json/,
      /check-node-audit-exceptions\.mjs --mode production/,
    ]),
    "security workflow portable gate: unfiltered production npm audit",
    "security workflow must run omit-dev npm audit through the no-exception production gate",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /npm audit --audit-level=high --json/,
      /check-node-audit-exceptions\.mjs --mode dev-codegen/,
    ]),
    "security workflow portable gate: fail-closed dev/codegen npm exceptions",
    "security workflow must run the full npm audit through the exact dev/codegen exception gate",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /trivy\s+fs/,
      /--ignorefile\s+security\/trivy-dev-codegen-exceptions\.yaml/,
    ]),
    "security workflow portable gate: scoped Trivy exceptions",
    "security workflow must scope full-scan Trivy exceptions to the checked-in YAML policy",
  );
  requirement(
    result,
    workflowHasRun(securityWorkflow, [
      /node scripts\/generate-trivy-dev-codegen-exceptions\.mjs --check/,
    ]),
    "security workflow portable gate: canonical Trivy exception projection",
    "security workflow must verify canonical Trivy exception parity before the full-scan YAML policy is used",
  );

  // Fail-closed wrapper-argv0 gate (console-ry4f mechanism replacement).
  //
  // The expected executed-test-source set is the source-derived inventory in the
  // executed-tests baseline. Without it the gate has no positive anchor, so it must
  // fail closed rather than silently stop defending the CI wiring (the same
  // "missing baseline must fail" posture as check-executed-tests.mjs).
  const executedTestsBaselinePath = "docs/program/executed-tests-baseline.json";
  let expectedExecutedSources = [];
  try {
    const executedTestsBaseline = JSON.parse(readText(executedTestsBaselinePath));
    const inventory = validExecutedSourceInventory(
      executedTestsBaseline?.test_attribute_baseline,
    );
    if (inventory === null) {
      result.failures.push(
        `wrapper-argv0 gate: ${executedTestsBaselinePath} test_attribute_baseline must be an object mapping source paths to non-negative integer counts`,
      );
    } else {
      expectedExecutedSources = inventory;
    }
  } catch (error) {
    result.failures.push(
      `wrapper-argv0 gate: ${executedTestsBaselinePath} must be valid JSON (${error.message})`,
    );
  }
  requirement(
    result,
    expectedExecutedSources.length > 0,
    "wrapper-argv0 gate: expected executed-test-source inventory present",
    `wrapper-argv0 gate: ${executedTestsBaselinePath} must name the expected executed-test sources (test_attribute_baseline)`,
  );
  for (const [workflowPath, workflowText] of [
    [ciPath, ciWorkflow],
    [securityPath, securityWorkflow],
    [imageReleasePath, imageReleaseWorkflow],
  ]) {
    const wrappers = workflowWrapperInvocations(workflowText);
    requirement(
      result,
      wrappers.length === 0,
      `wrapper-argv0 gate: ${workflowPath} runs test/check binaries directly (${expectedExecutedSources.length} expected executed sources)`,
      wrappers.map((finding) => wrapperFindingMessage(workflowPath, finding)).join("\n"),
    );
  }

  return result;
}

export function evaluateSmtpDeploymentChecks(readText) {
  const result = createResult();
  const configPath = "deploy/apps/console/base/configmap.yaml";
  const configMap = requirePresentText(
    result,
    readText,
    configPath,
    "maintenance runtime ConfigMap",
  );
  const activeRelayFields = activeYamlScalarKeys(
    configMap,
    SMTP_NON_SECRET_KEYS,
  );
  if (activeRelayFields.length === 0) {
    const stubMode = extractYamlScalar(
      stripHashComments(configMap),
      SMTP_STUB_MODE_KEY,
    )
      ?.trim()
      .toLowerCase();
    if (stubMode && SMTP_ALLOWED_STUB_MODES.includes(stubMode)) {
      result.passes.push(
        `SMTP relay disabled for explicit stub mode ${SMTP_STUB_MODE_KEY}=${stubMode}`,
      );
    } else {
      result.failures.push(
        `${configPath} must either configure non-secret CONSOLE_EMAIL_* SMTP relay fields or set ${SMTP_STUB_MODE_KEY}=local|dev|development|test|e2e for an explicit non-production stub email config`,
      );
    }
    return result;
  }

  const completeWorkloads = [];
  for (const workload of SMTP_WORKLOADS) {
    const text = requirePresentText(
      result,
      readText,
      workload.path,
      `${workload.label} workload manifest`,
    );
    const missingKeys = SMTP_SECRET_KEYS.filter(
      (key) => !envVarRequiresSecretKeyRef(text, key, "console-secrets"),
    );
    if (missingKeys.length === 0) {
      completeWorkloads.push(workload.label);
      continue;
    }
    for (const key of missingKeys) {
      result.failures.push(
        `${workload.path} must explicitly require ${key} from console-secrets via secretKeyRef when ${configPath} sets ${activeRelayFields.join(", ")}; envFrom alone does not fail on missing Secret keys before rollout`,
      );
    }
  }

  if (completeWorkloads.length === SMTP_WORKLOADS.length) {
    result.passes.push(
      `SMTP production credential refs: ${completeWorkloads.join(", ")}`,
    );
  }
  return result;
}

export function evaluateArgoTargetRevisionChecks(readText) {
  const result = createResult();
  for (const path of [
    "deploy/argocd/root.yaml",
    "deploy/argocd/apps/console.yaml",
  ]) {
    const text = requirePresentText(result, readText, path, path);
    const targetRevision = extractYamlScalar(
      stripHashComments(text),
      "targetRevision",
    );
    requirement(
      result,
      targetRevision === "main",
      `${path} tracks main`,
      `${path} must actively set targetRevision: main (found ${formatScalar(targetRevision)})`,
    );
  }
  return result;
}

export function evaluateDeployAutomationChecks(readText) {
  const result = createResult();
  const path = "scripts/deploy.sh";
  const deployScript = requirePresentText(
    result,
    readText,
    path,
    "deploy automation script",
  );
  const logicalLines = combineShellContinuations(
    extractExecutableShellLines(deployScript),
  );
  const executableText = logicalLines.join("\n");
  const rollouts = parseShellArray(logicalLines, "ROLLOUTS");
  const requiredRollouts = ["console-app"];
  const missingRollouts = requiredRollouts.filter(
    (rollout) => !rollouts.includes(rollout),
  );
  requirement(
    result,
    missingRollouts.length === 0,
    `deploy automation rollouts covered: ${requiredRollouts.join(", ")}`,
    `${path} must actively wait for the console-app rollout; ROLLOUTS must list it before claiming deployment verification (missing ${missingRollouts.join(", ") || "none"})`,
  );

  const failOpenBlocks = findFailOpenKubectlPrerequisiteBlocks(logicalLines);
  const hasKubectlRequire = logicalLines.some((line) =>
    /^require\s+kubectl\b/.test(line),
  );
  const kubectlCommandPrefix = /\b(?:kubectl|kubectl_required)\b/;
  const hasClusterReachabilityCheck = logicalLines.some(
    (line) => kubectlCommandPrefix.test(line) && /\bversion\b/.test(line),
  );
  requirement(
    result,
    hasKubectlRequire &&
      hasClusterReachabilityCheck &&
      failOpenBlocks.length === 0,
    "deploy automation kubectl prerequisite: fail-closed",
    `${path} must fail closed before endpoint checks when kubectl or the target cluster is unavailable`,
  );

  const hasActiveSkipPath =
    /\bskipp\w*\b[^\n]*(?:rollout|in-cluster|cluster)|(?:rollout|in-cluster|cluster)[^\n]*\bskipp\w*\b/i.test(
      executableText,
    );
  requirement(
    result,
    !hasActiveSkipPath,
    "deploy automation rollout skip path absent",
    `${path} must not contain an active rollout-skip path that can continue to endpoint/final success`,
  );

  const hasDigestBumpOnlyMode =
    /--digest-bump-only/.test(executableText) &&
    /--bump-only/.test(executableText) &&
    /MODE=.*digest-bump-only|digest-bump-only.*MODE/.test(executableText);
  const hasDigestBumpOnlyTruthfulMessage =
    /desired prod digests updated only/.test(executableText) &&
    /deployment, rollout, pod-image, and endpoint verification were NOT run/.test(
      executableText,
    );
  const digestBumpOnlyVerificationClaims =
    findDigestBumpOnlyVerificationClaims(logicalLines);
  requirement(
    result,
    hasDigestBumpOnlyMode &&
      hasDigestBumpOnlyTruthfulMessage &&
      digestBumpOnlyVerificationClaims.length === 0,
    "deploy automation digest-bump-only mode: truthful non-verification",
    `${path} digest-bump-only mode must not claim deployment, rollout, pod-image, or endpoint verification`,
  );

  if (/scripts\/bump-prod-digests\.sh/.test(executableText)) {
    const initialAuthorityIndex = executableText.indexOf(
      'scripts/check-production-promotion-authority.py" initial',
    );
    const mutationIndex = executableText.indexOf(
      "scripts/bump-prod-digests.sh",
    );
    const resetAuthorityIndex = executableText.indexOf(
      'scripts/check-production-promotion-authority.py" reset',
    );
    const commitIndex = executableText.indexOf("git commit");
    const prePushAuthorityIndex = executableText.indexOf(
      'scripts/check-production-promotion-authority.py" pre-push',
    );
    const pushIndex = executableText.indexOf(
      'git push origin "HEAD:${BRANCH}"',
    );
    const pushMatches =
      executableText.match(/git push origin "HEAD:\$\{BRANCH\}"/g) ?? [];
    const remoteChecks =
      executableText.match(
        /scripts\/check-production-promotion-authority\.py" remote/g,
      ) ?? [];
    requirement(
      result,
      initialAuthorityIndex >= 0 &&
        mutationIndex > initialAuthorityIndex &&
        resetAuthorityIndex > mutationIndex &&
        commitIndex > resetAuthorityIndex,
      "deploy automation production authorization precedes mutation and is consumed",
      `${path} must verify authorization before mutation and reset the one-shot authorization before commit`,
    );
    requirement(
      result,
      commitIndex >= 0 &&
        prePushAuthorityIndex > commitIndex &&
        pushIndex > prePushAuthorityIndex &&
        pushMatches.length === 1,
      "deploy automation immutable-dev recheck precedes push",
      `${path} must re-check origin/dev after commit and before its production push`,
    );
    requirement(
      result,
      remoteChecks.length >= 3 &&
        /verify_argo_pre_refresh_revision\s+"\$\{SHA\}"\s+"\$\{SHA\}"/.test(
          executableText,
        ) &&
        /verify_argo_pre_refresh_revision\s+"\$\{SHA\}"\s+"\$\{DEPLOY_REVISION\}"/.test(
          executableText,
        ),
      "deploy automation remote and Argo revisions are rechecked around refresh",
      `${path} must re-check exact remote and Argo revisions before refresh and after rollout/endpoint verification`,
    );
    requirement(
      result,
      !/\bgit\s+(?:pull|rebase)\b|\bfor\s+attempt\b|\bretr(?:y|ies|ied|ying)\b.{0,80}\bgit\s+push\b|\bgit\s+push\b.{0,80}\bretr(?:y|ies|ied|ying)\b/is.test(
        executableText,
      ),
      "deploy automation has no rebase or retry path",
      `${path} must not pull, rebase, retry, or loop after a racing dev push`,
    );
  }

  const hasArgoHardRefresh = logicalLines.some(
    (line) =>
      kubectlCommandPrefix.test(line) &&
      /\bannotate\b/.test(line) &&
      /argocd\.argoproj\.io\/refresh=hard/.test(line) &&
      /--overwrite\b/.test(line),
  );
  requirement(
    result,
    hasArgoHardRefresh,
    "deploy automation Argo hard refresh: active",
    `${path} must actively request an Argo hard refresh before rollout verification`,
  );

  const rolloutStatusLines = logicalLines.filter(
    (line) =>
      kubectlCommandPrefix.test(line) &&
      /\bargo\s+rollouts\s+status\b/.test(line),
  );
  requirement(
    result,
    rolloutStatusLines.length > 0,
    `deploy automation rollout status commands: ${rolloutStatusLines.length}`,
    `${path} must actively wait for kubectl argo rollouts status, not only mention it in comments or unused helpers`,
  );
  const swallowedRollouts = rolloutStatusLines.filter(
    (line) => /\|\|\s*(?:true|:)\b/.test(line) || /;\s*true\b/.test(line),
  );
  requirement(
    result,
    swallowedRollouts.length === 0 && !/\bset\s+\+e\b/.test(executableText),
    "deploy automation rollout failures are not swallowed",
    `${path} must not swallow rollout status failures with || true, :, or set +e`,
  );

  const rolloutIndex = logicalLines.findIndex(
    (line) =>
      kubectlCommandPrefix.test(line) &&
      /\bargo\s+rollouts\s+status\b/.test(line),
  );
  const endpointIndex = logicalLines.findIndex((line) => /\bcurl\b/.test(line));
  const finalSuccessIndex = logicalLines.findIndex((line) =>
    /deployed and verified/.test(line),
  );
  requirement(
    result,
    rolloutIndex >= 0 &&
      endpointIndex >= 0 &&
      finalSuccessIndex > rolloutIndex &&
      finalSuccessIndex > endpointIndex,
    "deploy automation final success follows rollout and endpoint checks",
    `${path} final deployed-and-verified message must occur after rollout and endpoint verification`,
  );

  return result;
}

export function evaluateGlobalHardeningChecks(readText) {
  const result = createResult();

  requirePackageScript(result, readText, "check:production-hardening");
  requirePackageScript(result, readText, "check:k8s");
  requirePackageScript(result, readText, "check:k8s:networkpolicy");
  appendResult(result, evaluateExpandContractReleaseChecks(readText));
  appendResult(result, evaluateWorkflowHardeningChecks(readText));
  requireTextIncludes(
    result,
    readText,
    ".github/workflows/ci.yml",
    "npm run check:k8s",
    "CI runs Kubernetes render/NetworkPolicy preflight contract",
  );

  requireTextIncludes(
    result,
    readText,
    "scripts/check-networkpolicy-enforcement.sh",
    "CONSOLE_NETWORKPOLICY_PREFLIGHT",
    "NetworkPolicy preflight has warning/required modes",
  );
  requireTextIncludes(
    result,
    readText,
    "scripts/check-networkpolicy-enforcement.sh",
    "policy-capable CNI",
    "NetworkPolicy preflight distinguishes manifest render from CNI enforcement",
  );
  requireTextIncludes(
    result,
    readText,
    "docs/CI-GATES.md",
    "CONSOLE_NETWORKPOLICY_PREFLIGHT=require npm run check:k8s:networkpolicy",
    "CI gates document required NetworkPolicy enforcement preflight",
  );
  requireTextIncludes(
    result,
    readText,
    "deploy/README.md",
    "CONSOLE_NETWORKPOLICY_PREFLIGHT=require npm run check:k8s:networkpolicy",
    "deployment checklist requires live NetworkPolicy enforcement preflight",
  );

  requireTextIncludes(
    result,
    readText,
    ".github/workflows/release-please.yml",
    "RELEASE_PLEASE_TOKEN",
    "release-please PR/token path documented",
  );

  appendResult(result, evaluateSmtpDeploymentChecks(readText));
  appendResult(result, evaluateProdOverlayImageChecks(readText));
  appendResult(result, evaluateArgoTargetRevisionChecks(readText));
  appendResult(result, evaluateDeployAutomationChecks(readText));

  requirePresentText(
    result,
    readText,
    "deploy/apps/console/components/admission-audit/kustomization.yaml",
    "admission-audit component",
  );
  requirePresentText(
    result,
    readText,
    "deploy/apps/console/components/admission-audit/README.md",
    "admission-audit runbook",
  );
  for (const needle of [
    "kind: ClusterImagePolicy",
    "mode: warn",
    "ghcr.io/jason931225/console-app",
    "https://token.actions.githubusercontent.com",
    "image-release\\.yml@refs/(heads/dev|tags/v[0-9].*)",
    "https://fulcio.sigstore.dev",
    "https://rekor.sigstore.dev",
  ]) {
    requireTextIncludes(
      result,
      readText,
      "deploy/apps/console/components/admission-audit/clusterimagepolicy.yaml",
      needle,
      `admission audit policy: ${needle}`,
    );
  }

  for (const needle of [
    "TimeoutLayer::with_status_code",
    "DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES)",
    "http_trace_layer()",
    "with_metrics(router, &state)",
    "http_route = %cardinality_safe_http_route(request)",
    "router_layer_tests",
    "default_request_timeout_is_thirty_seconds",
  ]) {
    requireTextIncludes(
      result,
      readText,
      "backend/app/src/lib.rs",
      needle,
      `backend request envelope: ${needle}`,
    );
  }
  requirePresentText(
    result,
    readText,
    "backend/app/slos/api-availability.openslo.yaml",
    "OpenSLO availability objective",
  );
  requirePresentText(
    result,
    readText,
    "backend/app/slos/api-latency.openslo.yaml",
    "OpenSLO latency objective",
  );
  requirePresentText(
    result,
    readText,
    "deploy/apps/console/components/monitoring/servicemonitor.yaml",
    "Prometheus ServiceMonitor",
  );
  requirePresentText(
    result,
    readText,
    "deploy/apps/console/components/monitoring/prometheusrule.yaml",
    "PrometheusRule SLO alerts",
  );
  for (const needle of [
    "/metrics",
    "ConsoleApiAvailabilityBurn",
    "ConsoleApiLatencyP99High",
    "Prometheus Operator",
  ]) {
    const file =
      needle === "/metrics"
        ? "deploy/apps/console/components/monitoring/servicemonitor.yaml"
        : needle === "Prometheus Operator"
          ? "deploy/apps/console/components/monitoring/README.md"
          : "deploy/apps/console/components/monitoring/prometheusrule.yaml";
    requireTextIncludes(
      result,
      readText,
      file,
      needle,
      `monitoring portable contract: ${needle}`,
    );
  }

  // Dark mox mail stack: internal-only StatefulSet, wired to the app over the
  // in-cluster webhook, network-fenced, observable, and never exposing a public
  // mail/admin surface.
  for (const needle of [
    "kind: StatefulSet",
    "name: console-mox",
    "r.xmox.nl/mox@sha256",
    "WebAPIHTTP",
    "MetricsHTTP",
    "volumeClaimTemplates",
  ]) {
    requireTextIncludes(
      result,
      readText,
      "deploy/apps/console/base/mox.yaml",
      needle,
      `mox dark stack: ${needle}`,
    );
  }
  for (const needle of [
    "CONSOLE_MAIL_MOX_BASE_URL",
    "http://console-mox.console.svc:1080",
  ]) {
    requireTextIncludes(
      result,
      readText,
      "deploy/apps/console/base/configmap.yaml",
      needle,
      `mox app wiring: ${needle}`,
    );
  }
  for (const needle of [
    "allow-app-egress-mox",
    "allow-mox-ingress-internal",
    "default-deny-egress-mox",
    "allow-mox-egress-app-webhook",
  ]) {
    requireTextIncludes(
      result,
      readText,
      "deploy/apps/console/base/networkpolicy.yaml",
      needle,
      `mox network policy: ${needle}`,
    );
  }
  for (const needle of [
    "name: console-mox",
    "port: metrics",
    "ConsoleMoxDown",
    "ConsoleMoxWebhookFailures",
    "ConsoleMoxQueueBacklog",
    "ConsoleMoxPvcSaturation",
  ]) {
    const file =
      needle === "port: metrics" || needle === "name: console-mox"
        ? "deploy/apps/console/components/monitoring/servicemonitor.yaml"
        : "deploy/apps/console/components/monitoring/prometheusrule.yaml";
    requireTextIncludes(
      result,
      readText,
      file,
      needle,
      `mox observability: ${needle}`,
    );
  }
  const moxManifest = readText("deploy/apps/console/base/mox.yaml");
  for (const forbidden of [
    "NodePort",
    "LoadBalancer",
    "port: 25",
    "AdminHTTP",
    "Submission:",
    "Submissions:",
  ]) {
    requirement(
      result,
      !moxManifest.includes(forbidden),
      `mox dark stack excludes ${forbidden}`,
      `mox dark stack must not expose public mail/admin surface: found ${forbidden}`,
    );
  }

  return result;
}

export function evaluateOciGuestCnpgChecks(readText) {
  const result = createResult();
  const paths = {
    base: "deploy/apps/console/base/database.yaml",
    prod: "deploy/apps/console/overlays/prod/kustomization.yaml",
  };

  const baseDatabase = readText(paths.base);
  const baseClusterEnvNames = extractYamlSequenceItemNames(
    baseDatabase,
    "env",
    2,
  );
  const baseInstancesValue = extractYamlScalar(baseDatabase, "instances");
  const baseInstances = parsePositiveInteger(baseInstancesValue);
  requirement(
    result,
    baseInstances === 1,
    `oci-guest CNPG base instances: ${baseInstances}`,
    `oci-guest CNPG base instances: ${paths.base} must keep the live single-node context at spec.instances: 1 (found ${formatScalar(baseInstancesValue)})`,
  );
  requirement(
    result,
    !/^\s*storageClass\s*:/m.test(baseDatabase),
    "oci-guest CNPG base storage: default/local-path-compatible (no pinned storageClass)",
    `oci-guest CNPG base storage: ${paths.base} must not pin the on-prem replicated storageClass; keep storageClass selection context-specific`,
  );
  const hasOciChecksumEnvOverrides = CNPG_OCI_CHECKSUM_ENV_NAMES.every(
    (name) => {
      const block = extractEnvVarBlock(baseDatabase, name);
      return (
        baseClusterEnvNames.includes(name) &&
        /^\s*value:\s*when_required\s*$/m.test(block)
      );
    },
  );
  requirement(
    result,
    hasOciChecksumEnvOverrides,
    "oci-guest CNPG checksum workaround: retained for OCI Object Storage",
    `oci-guest CNPG checksum workaround: ${paths.base} must retain the OCI-only AWS checksum env overrides`,
  );

  const prodOverlay = readText(paths.prod);
  requirement(
    result,
    prodOverlay.includes("../../base"),
    "oci-guest prod overlay: inherits maintenance base",
    `oci-guest prod overlay: ${paths.prod} must inherit ../../base before applying live production patches`,
  );
  requirement(
    result,
    !prodOverlay.includes("cnpg-ha-patch.yaml") &&
      !prodOverlay.includes("console-pg-hot"),
    "oci-guest prod overlay: does not opt into on-prem HA storage patch",
    `oci-guest prod overlay: ${paths.prod} must not load cnpg-ha-patch.yaml or console-pg-hot; those belong to the on-prem-ha context`,
  );
  requirement(
    result,
    !/(\/spec\/instances|\/spec\/storage\/storageClass|^\s*instances:\s*(?:[2-9]|\d{2,})|^\s*storageClass\s*:)/m.test(
      prodOverlay,
    ),
    "oci-guest prod overlay CNPG shape: does not patch instances or storageClass",
    `oci-guest prod overlay CNPG shape: ${paths.prod} must leave CNPG instances/storage on the single-node base; use deploy/apps/console/overlays/on-prem for HA`,
  );

  return result;
}

export function evaluateOnPremHaCnpgChecks(readText) {
  const result = createResult();
  const paths = {
    base: "deploy/apps/console/base/database.yaml",
    onPrem: "deploy/apps/console/overlays/on-prem/kustomization.yaml",
    onPremPatch: "deploy/apps/console/overlays/on-prem/cnpg-ha-patch.yaml",
    storageClass: "deploy/apps/storage/manifests/storageclass-console-pg-hot.yaml",
  };

  const onPremOverlay = readText(paths.onPrem);
  requirement(
    result,
    onPremOverlay.includes("../../base") &&
      onPremOverlay.includes("cnpg-ha-patch.yaml"),
    "on-prem-ha CNPG overlay path: patches the base cluster",
    `on-prem-ha CNPG overlay path: ${paths.onPrem} must inherit ../../base and include cnpg-ha-patch.yaml`,
  );
  requirement(
    result,
    /kind:\s*Cluster/.test(onPremOverlay) &&
      /name:\s*console-db/.test(onPremOverlay),
    "on-prem-ha CNPG overlay target: Cluster/console-db",
    `on-prem-ha CNPG overlay target: ${paths.onPrem} must target Cluster/console-db`,
  );

  const onPremEndpoint = extractJson6902PatchScalar(
    onPremOverlay,
    "/spec/configuration/endpointURL",
  );
  requirement(
    result,
    Boolean(onPremEndpoint) &&
      !/objectstorage\..*oraclecloud\.com/.test(onPremEndpoint),
    `on-prem-ha CNPG object-store endpoint: ${onPremEndpoint}`,
    `on-prem-ha CNPG object-store endpoint: ${paths.onPrem} must patch /spec/configuration/endpointURL to a self-hosted S3 endpoint, not OCI Object Storage (found ${formatScalar(onPremEndpoint)})`,
  );

  const onPremPatch = readText(paths.onPremPatch);
  const baseDatabase = readText(paths.base);
  const baseClusterEnvNames = extractYamlSequenceItemNames(
    baseDatabase,
    "env",
    2,
  );
  const nonChecksumBaseEnvNames = baseClusterEnvNames.filter(
    (name) => !CNPG_OCI_CHECKSUM_ENV_NAMES.includes(name),
  );
  const removesSpecEnv = /-\s*op:\s*remove[\s\S]*?path:\s*\/spec\/env/.test(
    onPremPatch,
  );
  requirement(
    result,
    removesSpecEnv,
    "on-prem-ha CNPG checksum behavior: removes inherited OCI-only AWS checksum env overrides",
    `on-prem-ha CNPG checksum behavior: ${paths.onPremPatch} must remove /spec/env so self-hosted S3 does not inherit the OCI-only AWS checksum workaround`,
  );
  requirement(
    result,
    !removesSpecEnv || nonChecksumBaseEnvNames.length === 0,
    "on-prem-ha CNPG env removal scope: base env contains only OCI checksum overrides",
    `on-prem-ha CNPG env removal scope: ${paths.onPremPatch} may remove /spec/env only while ${paths.base} env contains only OCI checksum overrides; extra base env entries would be dropped: ${nonChecksumBaseEnvNames.join(", ")}`,
  );
  const haInstancesValue = extractJson6902PatchScalar(
    onPremPatch,
    "/spec/instances",
  );
  const haInstances = parsePositiveInteger(haInstancesValue);
  requirement(
    result,
    haInstances !== undefined && haInstances >= 3,
    `on-prem-ha CNPG HA instances: ${haInstances}`,
    `on-prem-ha CNPG HA instances: ${paths.onPremPatch} must set /spec/instances to >= 3 (found ${formatScalar(haInstancesValue)})`,
  );

  const haStorageClass = extractJson6902PatchScalar(
    onPremPatch,
    "/spec/storage/storageClass",
  );
  requirement(
    result,
    haStorageClass === "console-pg-hot",
    "on-prem-ha CNPG HA storageClass: console-pg-hot",
    `on-prem-ha CNPG HA storageClass: ${paths.onPremPatch} must use console-pg-hot and must not use local-path (found ${formatScalar(haStorageClass)})`,
  );
  requirement(
    result,
    json6902PatchHasPath(onPremPatch, "/spec/postgresql/synchronous") &&
      /failoverQuorum:\s*true/.test(onPremPatch),
    "on-prem-ha CNPG HA synchronous replication/failover posture: present",
    `on-prem-ha CNPG HA synchronous replication/failover posture: ${paths.onPremPatch} must add /spec/postgresql/synchronous with failoverQuorum before claiming HA`,
  );
  requirement(
    result,
    /topologyKey:\s*kubernetes\.io\/hostname/.test(onPremPatch) ||
      /nodeLabelsAntiAffinity:\s*\n\s*-\s*kubernetes\.io\/hostname/.test(
        onPremPatch,
      ),
    "on-prem-ha CNPG HA scheduling spread: hostname anti-affinity/topology spread present",
    `on-prem-ha CNPG HA scheduling spread: ${paths.onPremPatch} must include anti-affinity or topology spread by hostname or stronger failure-domain labels`,
  );

  const storageClass = readText(paths.storageClass);
  requirement(
    result,
    /kind:\s*StorageClass/.test(storageClass) &&
      /^\s*name:\s*console-pg-hot\s*$/m.test(storageClass),
    "on-prem-ha storage contract: StorageClass/console-pg-hot",
    `on-prem-ha storage contract: ${paths.storageClass} must define StorageClass/console-pg-hot`,
  );
  const provisioner = extractYamlScalar(storageClass, "provisioner");
  requirement(
    result,
    provisioner === "driver.longhorn.io",
    "on-prem-ha storage provisioner: driver.longhorn.io",
    `on-prem-ha storage provisioner: ${paths.storageClass} must use replicated Longhorn storage, not local-path (found ${formatScalar(provisioner)})`,
  );
  const replicaCountValue = extractYamlScalar(storageClass, "numberOfReplicas");
  const replicaCount = parsePositiveInteger(replicaCountValue);
  requirement(
    result,
    replicaCount !== undefined && replicaCount >= 3,
    `on-prem-ha storage replicas: ${replicaCount}`,
    `on-prem-ha storage replicas: ${paths.storageClass} must set numberOfReplicas to >= 3 (found ${formatScalar(replicaCountValue)})`,
  );
  requirement(
    result,
    /^\s*reclaimPolicy:\s*Retain\s*$/m.test(storageClass),
    "on-prem-ha storage reclaim policy: Retain",
    `on-prem-ha storage reclaim policy: ${paths.storageClass} must use Retain to avoid accidental PostgreSQL data loss`,
  );
  requirement(
    result,
    /^\s*volumeBindingMode:\s*WaitForFirstConsumer\s*$/m.test(storageClass),
    "on-prem-ha storage binding mode: WaitForFirstConsumer",
    `on-prem-ha storage binding mode: ${paths.storageClass} must use WaitForFirstConsumer for topology-aware replicated storage placement`,
  );

  return result;
}

export function evaluateCnpgContextChecks(readText) {
  const result = createResult();
  appendResult(result, evaluateOciGuestCnpgChecks(readText));
  appendResult(result, evaluateOnPremHaCnpgChecks(readText));
  return result;
}

export function evaluateOciGuestContextChecks(readText) {
  const result = createResult();
  appendResult(result, evaluateOciGuestCnpgChecks(readText));

  const runbookPath = "deploy/OPS-RUNBOOK.md";
  const runbook = requirePresentText(
    result,
    readText,
    runbookPath,
    "oci-guest runbook",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /`oci-guest` runbook|live OCI\/Talos cluster/i,
    "oci-guest runbook identity: explicit",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /OCI Vault.*(recover|secret|credential)|Everything needed to recover.*OCI Vault/is,
    "oci-guest secret recovery source: OCI Vault documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /Ampere A1|A1 node|free-tier\s+A1/i,
    "oci-guest topology: A1 single-node substrate documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /single control-plane node|one schedulable control-plane|control-plane node \(schedules workloads\)/i,
    "oci-guest topology: one schedulable control-plane documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /Reserved public IP|140\.245\.68\.253/,
    "oci-guest topology: reserved IP documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /second\s+A1|A1 allotment|Free-tier guardrails/i,
    "oci-guest free-tier guardrail: no accidental second A1",
  );

  const secretsPath = "deploy/SECRETS.md";
  const secrets = requirePresentText(
    result,
    readText,
    secretsPath,
    "oci-guest secrets runbook",
  );
  requireRegexInText(
    result,
    secretsPath,
    secrets,
    /OCI Vault/i,
    "oci-guest secrets source: OCI Vault/manual bootstrap documented",
  );
  requireRegexInText(
    result,
    secretsPath,
    secrets,
    /External\s+Secrets|Sealed\s+Secrets/i,
    "oci-guest secrets upgrade path: External Secrets or Sealed Secrets documented",
  );
  requireIncludesInText(
    result,
    secretsPath,
    secrets,
    "CONSOLE_MAIL_MASTER_KEY",
    "oci-guest secrets runbook: mail KEK remains documented",
  );

  const databasePath = "deploy/apps/console/base/database.yaml";
  const database = requirePresentText(
    result,
    readText,
    databasePath,
    "oci-guest database/ObjectStore manifest",
  );
  requireIncludesInText(
    result,
    databasePath,
    database,
    "kind: ObjectStore",
    "oci-guest CNPG backup object: ObjectStore present",
  );
  const destinationPath = extractYamlScalar(database, "destinationPath");
  requirement(
    result,
    destinationPath === "s3://mnt-db-backups/",
    "oci-guest CNPG backup bucket: s3://mnt-db-backups/",
    `oci-guest CNPG backup bucket: ${databasePath} must set destinationPath: s3://mnt-db-backups/ (found ${formatScalar(destinationPath)})`,
  );
  const endpointUrl = extractYamlScalar(database, "endpointURL");
  requirement(
    result,
    /compat\.objectstorage\..*oraclecloud\.com/.test(endpointUrl ?? ""),
    `oci-guest CNPG backup endpoint: ${endpointUrl}`,
    `oci-guest CNPG backup endpoint: ${databasePath} must use the OCI Object Storage S3-compatible endpoint for the oci-guest context (found ${formatScalar(endpointUrl)})`,
  );
  for (const needle of [
    "oci-objectstore-creds",
    "ACCESS_KEY_ID",
    "ACCESS_SECRET_KEY",
    "kind: ScheduledBackup",
  ]) {
    requireIncludesInText(
      result,
      databasePath,
      database,
      needle,
      `oci-guest CNPG backup credential/schedule: ${needle}`,
    );
  }
  requireRegexInText(
    result,
    databasePath,
    database,
    /retentionPolicy|Indefinite retention|never prunes|no retentionPolicy/i,
    "oci-guest CNPG backup retention posture: explicit",
  );
  requireRegexInText(
    result,
    databasePath,
    database,
    /AWS_REQUEST_CHECKSUM_CALCULATION[\s\S]*when_required[\s\S]*OCI Object Storage/i,
    "oci-guest OCI-specific Barman checksum workaround: labeled and scoped",
  );

  const enterprisePath = "docs/ENTERPRISE-READINESS.md";
  const enterprise = requirePresentText(
    result,
    readText,
    enterprisePath,
    "oci-guest enterprise readiness note",
  );
  requireRegexInText(
    result,
    enterprisePath,
    enterprise,
    /`oci-guest`[\s\S]*single(?:-|\s)node|single free-tier node/i,
    "oci-guest HA honesty: single-node posture documented",
  );
  requireRegexInText(
    result,
    enterprisePath,
    enterprise,
    /restore-from-backup event|not an automatic failover/i,
    "oci-guest HA honesty: restore-not-failover documented",
  );

  const imageReleasePath = ".github/workflows/image-release.yml";
  const imageRelease = requirePresentText(
    result,
    readText,
    imageReleasePath,
    "oci-guest image-release workflow",
  );
  requireRegexInText(
    result,
    imageReleasePath,
    imageRelease,
    /(?:target|platforms):\s*linux\/arm64/,
    "oci-guest image platform: current arm64 target explicit",
  );
  requireRegexInText(
    result,
    imageReleasePath,
    imageRelease,
    /A1 cluster|Ampere A1|Oracle Ampere/i,
    "oci-guest image platform: arm64 rationale scoped to OCI/A1",
  );

  const drPath = "ops/dr/DR-POLICY.md";
  const drPolicy = requirePresentText(
    result,
    readText,
    drPath,
    "oci-guest DR policy",
  );
  for (const needle of [
    "RPO: <= 5 minutes",
    "RTO: <= 1 hour",
    "pitr_drill_complete=ok",
  ]) {
    requireIncludesInText(
      result,
      drPath,
      drPolicy,
      needle,
      `oci-guest DR policy: ${needle}`,
    );
  }

  return result;
}

export function evaluateOnPremHaContextChecks(readText) {
  const result = createResult();
  appendResult(result, evaluateOnPremHaCnpgChecks(readText));

  const runbookPath = "deploy/OPS-RUNBOOK-baremetal.md";
  const runbook = requirePresentText(
    result,
    readText,
    runbookPath,
    "on-prem-ha runbook",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /\bADR-0024\b/,
    "on-prem-ha runbook identity: explicit ADR-0024",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /OpenBao[\s\S]*secret root|secret root[\s\S]*OpenBao/i,
    "on-prem-ha secret root: OpenBao documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /External Secrets Operator|External Secrets/i,
    "on-prem-ha secret projection: External Secrets documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /unseal[\s\S]*(audit|backup|snapshot)|audit[\s\S]*(backup|snapshot)/i,
    "on-prem-ha secret operations: unseal/audit/backup expectations documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /Do not paste|keep secret values out of git|out-of-band encrypted backup/i,
    "on-prem-ha secret handling: no committed/pasted unseal or root material",
  );

  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /SeaweedFS/i,
    "on-prem-ha object store: accepted self-hosted S3 reference documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /CNPG Barman[\s\S]*(endpoint|S3 URL)|Barman[\s\S]*on-prem S3/i,
    "on-prem-ha object store: CNPG Barman endpoint requirements documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /credentials from OpenBao\/ESO|OpenBao\/ESO[\s\S]*bucket names|TLS CA material/i,
    "on-prem-ha object store: credentials/buckets/TLS from portable secret path documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /second physical site|independent failure domain/i,
    "on-prem-ha object store: independent retention/replication failure domain documented",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /not copy[\s\S]*AWS_\*_CHECKSUM|AWS_\*_CHECKSUM[\s\S]*blindly/i,
    "on-prem-ha object store: OCI checksum workaround not blindly copied",
  );

  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /three (?:named )?control-plane|three control-plane\/etcd|three healthy etcd/i,
    "on-prem-ha topology: three control-plane/etcd members required",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /dedicated worker|worker\/storage nodes|failure domains/i,
    "on-prem-ha topology: worker/storage failure domains required",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /stable Kubernetes API endpoint|CONTROL_PLANE_VIP|VIP/i,
    "on-prem-ha topology: stable API endpoint/VIP required",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /Site NTP|site NTP|real fabric MTU|fabric MTU/i,
    "on-prem-ha topology: site NTP and real fabric MTU required",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /Never reuse the OCI public IP|no OCI IP|Do not copy OCI/i,
    "on-prem-ha topology: no OCI IP/hostPort assumptions",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /DARK|not wired|manual-sync|founder\/operator activation/i,
    "on-prem-ha DARK boundary: no live Argo cutover without operator activation",
  );
  requireRegexInText(
    result,
    runbookPath,
    runbook,
    /multi-arch images|x86_64|amd64|extend `platforms`/i,
    "on-prem-ha image platform: multi-arch/x86 readiness decision documented",
  );

  const enterprisePath = "docs/ENTERPRISE-READINESS.md";
  const enterprise = requirePresentText(
    result,
    readText,
    enterprisePath,
    "on-prem-ha enterprise readiness note",
  );
  requireRegexInText(
    result,
    enterprisePath,
    enterprise,
    /on-prem[\s\S]*three Talos control-plane nodes/i,
    "on-prem-ha readiness: three Talos control-plane nodes documented",
  );
  requireRegexInText(
    result,
    enterprisePath,
    enterprise,
    /CNPG `instances: 3`|replicated storage[\s\S]*CNPG/i,
    "on-prem-ha readiness: replicated storage and CNPG instances: 3 documented",
  );
  requireRegexInText(
    result,
    enterprisePath,
    enterprise,
    /DARK docs\/manifests are readiness inputs|DARK artifacts/i,
    "on-prem-ha readiness: DARK artifacts are not live HA evidence",
  );

  const storageReadmePath = "deploy/apps/storage/README.md";
  const storageReadme = requirePresentText(
    result,
    readText,
    storageReadmePath,
    "on-prem-ha storage runbook",
  );
  requireRegexInText(
    result,
    storageReadmePath,
    storageReadme,
    /not sync this app into the current `oci-guest`|live OCI guest/i,
    "on-prem-ha storage runbook: not synced into current OCI guest",
  );
  requireRegexInText(
    result,
    storageReadmePath,
    storageReadme,
    /three eligible Kubernetes worker\/storage nodes|failure domains/i,
    "on-prem-ha storage runbook: three storage failure domains before activation",
  );

  const observabilityPath = "deploy/apps/observability/README.md";
  const observability = requirePresentText(
    result,
    readText,
    observabilityPath,
    "on-prem-ha observability runbook",
  );
  requireRegexInText(
    result,
    observabilityPath,
    observability,
    /OpenTelemetry Collector|VictoriaMetrics|Loki|Tempo|Grafana/i,
    "on-prem-ha observability: self-hosted telemetry stack staged",
  );
  requireRegexInText(
    result,
    observabilityPath,
    observability,
    /Do not replace[\s\S]*components\/monitoring|ServiceMonitor\/console-app[\s\S]*\/metrics/i,
    "on-prem-ha observability: preserves portable /metrics and monitoring contract",
  );

  return result;
}

export const DEPLOYMENT_CONTEXTS = Object.freeze([
  {
    id: "oci-guest",
    status: "live/current production substrate",
    evaluate: evaluateOciGuestContextChecks,
  },
  {
    id: "on-prem-ha",
    status: "DARK/additive ADR-0024 HA target",
    evaluate: evaluateOnPremHaContextChecks,
  },
]);

function runCli() {
  const groups = [
    { id: "global", result: evaluateGlobalHardeningChecks(read) },
    { id: "action-pins", result: evaluateActionPinChecks(read) },
    ...DEPLOYMENT_CONTEXTS.map((context) => ({
      id: context.id,
      result: context.evaluate(read),
    })),
  ];
  const failureGroups = groups.filter(
    (group) => group.result.failures.length > 0,
  );
  if (failureGroups.length) {
    const lines = [];
    for (const group of failureGroups) {
      lines.push(`${group.id}:`);
      lines.push(...group.result.failures.map((failure) => `- ${failure}`));
    }
    console.error(`Production hardening check failed:\n${lines.join("\n")}`);
    process.exit(1);
  }

  const passCount = groups.reduce(
    (sum, group) => sum + group.result.passes.length,
    0,
  );
  console.log(
    `Production hardening check passed (${passCount} checks across ${groups.length} groups).`,
  );
  for (const group of groups) {
    console.log(`${group.id}:`);
    for (const pass of group.result.passes) {
      console.log(`- ${pass}`);
    }
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  runCli();
}
