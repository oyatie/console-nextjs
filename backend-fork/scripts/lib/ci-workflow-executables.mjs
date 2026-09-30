// Minimal GitHub Actions + shell command extraction for reachability gates.
//
// This intentionally does not pretend YAML text is executable merely because it
// contains a command-shaped substring. Only `run:` bodies from jobs/steps that can
// gate CI are admitted, shell comments and continuations are resolved, and each
// returned entry is one actual command segment. A line such as
// `echo cargo test ...`, or a step guarded by literal `if: false`, therefore cannot
// manufacture test reachability.

function jobBlocks(workflow) {
  // Match the top-level `jobs:` key even when it carries a trailing YAML comment
  // (`jobs: # scanner bypass`), which is still a valid key.
  const jobsStart = workflow.search(/^jobs:/m);
  if (jobsStart < 0) return [];
  const jobsLineEnd = workflow.indexOf("\n", jobsStart);
  if (jobsLineEnd < 0) return [];
  const jobs = workflow.slice(jobsLineEnd + 1);
  return [...jobs.matchAll(/^  ([A-Za-z0-9_-]+):\n([\s\S]*?)(?=^  [A-Za-z0-9_-]+:|(?![\s\S]))/gm)]
    .map(([, name, block]) => ({ name, block }));
}

function stepBlocks(block) {
  // `steps:` may carry a trailing YAML comment (`steps: # comment`) and still be valid.
  const steps = block.match(/^    steps:\s*(?:#.*)?\n([\s\S]*)$/m)?.[1] ?? "";
  return steps.split(/^      - /m).slice(1);
}

const BLOCK_SCALAR_INDICATOR = /^[|>](?:[+-][1-9]?|[1-9][+-]?)?$/;

/**
 * YAML folded-scalar semantics: a line break between two non-empty lines folds into a
 * single space, and a break adjacent to an empty line stays a paragraph break. A folded
 * body must be reassembled BEFORE tokenizing, or a wrapper and its target that sit on
 * adjacent lines are misread as two unrelated commands.
 */
function foldBlockScalar(body) {
  const folded = [];
  let current = "";
  for (const line of body) {
    if (line === "") {
      if (current !== "") {
        folded.push(current);
        current = "";
      }
      folded.push("");
    } else {
      current += (current === "" ? "" : " ") + line;
    }
  }
  if (current !== "") folded.push(current);
  return folded.join("\n");
}

/** Strip a trailing ` # ...` YAML comment from a single-line scalar value. */
function stripYamlInlineComment(value) {
  const index = value.search(/\s#/);
  return (index < 0 ? value : value.slice(0, index)).trim();
}

/** Decode the YAML double-quoted scalar escape sequences into their characters. */
function decodeYamlEscapes(value) {
  const singles = { "0": "\0", a: "\x07", b: "\b", e: "\x1b", f: "\f", n: "\n", r: "\r", t: "\t", v: "\v", " ": " ", "\\": "\\", '"': '"', "/": "/" };
  return value
    .replace(/\\U([0-9A-Fa-f]{8})/g, (_, h) => String.fromCodePoint(parseInt(h, 16)))
    .replace(/\\u([0-9A-Fa-f]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)))
    .replace(/\\x([0-9A-Fa-f]{2})/g, (_, h) => String.fromCharCode(parseInt(h, 16)))
    .replace(/\\([0abefnrtv \\"\/])/g, (_, c) => (c in singles ? singles[c] : c));
}

/** Strip one pair of matching YAML flow scalar quotes and decode double-quoted escapes. */
function unquoteYamlScalar(value) {
  if (value.length < 2) return value;
  const first = value[0];
  const last = value[value.length - 1];
  if (first === '"' && last === '"') return decodeYamlEscapes(value.slice(1, -1));
  if (first === "'" && last === "'") return value.slice(1, -1).replace(/''/g, "'");
  return value;
}

function runScript(step) {
  const lines = step.split(/\r?\n/);
  const runLineIndex = lines.findIndex((line) => /^        run:/.test(line));
  if (runLineIndex < 0) return "";
  const value = unquoteYamlScalar(stripYamlInlineComment(lines[runLineIndex].slice("        run:".length).trim()));
  if (value === "") return "";
  if (!BLOCK_SCALAR_INDICATOR.test(value)) return value;
  const folded = value.startsWith(">");
  const body = [];
  for (const line of lines.slice(runLineIndex + 1)) {
    // YAML permits physically empty lines inside a block scalar. Do not let one truncate
    // the executable surface: the command after a visual paragraph break still runs.
    // `|`, `|-`, `|+`, `>`, `>-`, and `>+` are all block scalars; chomping only affects
    // trailing newlines, and folding only joins non-empty lines, neither of which hides
    // the argv0 of a wrapper. A trailing `# comment` on the `run:` line is a YAML
    // comment, not part of the indicator.
    if (line === "") {
      body.push("");
    } else if (line.startsWith("          ")) {
      body.push(line.slice(10));
    } else {
      break;
    }
  }
  return folded ? foldBlockScalar(body) : body.join("\n");
}

/**
 * `defaults.run.shell` at the given indentation (`defaults:` at `indent`, `run:` at
 * `indent + 2`, keys at `indent + 4`), or null when the defaults run block names no shell.
 */
function defaultsRunShell(block, indent) {
  const lines = block.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    if (lines[i] !== `${" ".repeat(indent)}defaults:`) continue;
    for (let j = i + 1; j < lines.length; j += 1) {
      if (lines[j] === `${" ".repeat(indent + 2)}run:`) {
        for (let k = j + 1; k < lines.length; k += 1) {
          const key = lines[k];
          if (key.startsWith(`${" ".repeat(indent + 4)}shell:`)) {
            return key.slice(`${" ".repeat(indent + 4)}shell:`.length).trim();
          }
          if (key.trim() === "") continue;
          if (!key.startsWith(`${" ".repeat(indent + 4)}`)) return null;
        }
        return null;
      }
      if (lines[j].trim() === "") continue;
      if (!lines[j].startsWith(`${" ".repeat(indent + 2)}`)) break;
    }
  }
  return null;
}

function yamlValue(block, key, indentation) {
  return block.match(new RegExp(`^${" ".repeat(indentation)}${key}: ([^\\n]+)$`, "m"))?.[1]?.trim();
}

function literalFalse(value) {
  if (!value) return false;
  const normalized = value.replace(/^\$\{\{\s*|\s*\}\}$/g, "").trim().toLowerCase();
  return normalized === "false" || normalized === "0" || normalized === "null";
}

function canGate(block, indentation) {
  if (literalFalse(yamlValue(block, "if", indentation))) return false;
  const continueOnError = yamlValue(block, "continue-on-error", indentation);
  return continueOnError === undefined || literalFalse(continueOnError);
}

function stripShellComment(line) {
  let quote = null;
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (escaped) {
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (quote) {
      if (character === quote) quote = null;
    } else if (character === "'" || character === '"') {
      quote = character;
    } else if (character === "#" && (index === 0 || /\s/.test(line[index - 1]))) {
      return line.slice(0, index);
    }
  }
  return line;
}

/** Find a shell here-document operator `<<`/`<<-` that sits OUTSIDE quotes, or null. */
function hereDocStart(line) {
  let quote = null;
  let escaped = false;
  for (let index = 0; index < line.length - 1; index += 1) {
    const character = line[index];
    if (escaped) {
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (quote) {
      if (character === quote) quote = null;
    } else if (character === "'" || character === '"') {
      quote = character;
    } else if (character === "<" && line[index + 1] === "<" && line[index + 2] !== "<") {
      const rest = line.slice(index + 2);
      const match = /^-?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?/.exec(rest);
      if (match) return { index, delimiter: match[1], end: index + 2 + match[0].length };
    }
  }
  return null;
}

export function shellCommandTokens(script) {
  const commands = [];
  let command = "";
  let hereDocDelimiter = null;
  for (const physicalLine of script.split(/\r?\n/)) {
    // Skip here-document data: `cmd <<'EOF'` ... `EOF` writes text to a command's stdin;
    // that data is not executable and must not be classified as commands. A `<<` inside
    // quotes (e.g. the GITHUB_ENV `echo "VAR<<EOF"` marker) is data, not a here-doc.
    if (hereDocDelimiter !== null) {
      if (physicalLine.trim() === hereDocDelimiter) hereDocDelimiter = null;
      continue;
    }
    const line = stripShellComment(physicalLine.trim());
    if (!line) continue;
    const hereDoc = hereDocStart(line);
    if (hereDoc) {
      hereDocDelimiter = hereDoc.delimiter;
      // The redirection `<<'EOF'` feeds data to the command on its left, but operators
      // and commands on its right (`cat <<'EOF' && cosign ...`) still execute after the
      // here-document is consumed, so preserve them.
      const remainder = `${line.slice(0, hereDoc.index)} ${line.slice(hereDoc.end)}`.trim();
      if (remainder) {
        commands.push((command ? `${command} ` : "") + remainder);
        command = "";
      }
      continue;
    }
    const continued = /\\\s*$/.test(line);
    command += (command ? " " : "") + (continued ? line.replace(/\\\s*$/, "") : line);
    if (!continued) {
      commands.push(command);
      command = "";
    }
  }
  if (command) commands.push(`${command}\\`);

  return commands.map((source) => {
    const tokens = [];
    let token = "";
    let quote = null;
    let escaped = false;
    const flush = () => {
      if (token) tokens.push(token);
      token = "";
    };
    for (let index = 0; index < source.length; index += 1) {
      const character = source[index];
      if (escaped) {
        token += character;
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (quote) {
        if (character === quote) quote = null;
        else token += character;
      } else if (character === "'" || character === '"') {
        quote = character;
      } else if (/\s/.test(character)) {
        flush();
      } else if (";|&".includes(character)) {
        flush();
        if ((character === "|" || character === "&") && source[index + 1] === character) {
          tokens.push(`${character}${character}`);
          index += 1;
        } else {
          tokens.push(character);
        }
      } else {
        token += character;
      }
    }
    const malformed = quote !== null || escaped;
    if (escaped) token += "\\";
    flush();
    return { source, tokens, malformed };
  });
}

function commandSegments(surface) {
  const segments = [];
  let segment = [];
  let conditional = false;
  for (const token of [...surface.tokens, ";"]) {
    if ([";", "|", "&", "||", "&&"].includes(token)) {
      if (segment.length > 0) segments.push({ tokens: segment, conditional });
      segment = [];
      // `;` is a sequence operator (the next segment runs regardless); `&&`, `||`,
      // `|`, and `&` make the next segment conditional on the previous one.
      conditional = token !== ";";
    } else {
      segment.push(token);
    }
  }
  return segments;
}

function collectWorkflowCommands(workflow, includeNonGating) {
  const commands = [];
  const workflowShell = defaultsRunShell(workflow, 0);
  for (const job of jobBlocks(workflow)) {
    const jobGating = canGate(job.block, 4);
    const jobShell = yamlValue(job.block, "shell", 4) ?? defaultsRunShell(job.block, 4);
    for (const step of stepBlocks(job.block)) {
      const gating = jobGating && canGate(step, 8);
      if (!includeNonGating && !gating) continue;
      const script = runScript(step);
      if (!script) continue;
      // Effective shell: step override, else job default (direct or defaults.run),
      // else workflow defaults.run, else the runner default.
      const shell = yamlValue(step, "shell", 8) ?? jobShell ?? workflowShell ?? null;
      // A fail-slow keep-going block collects per-invocation failures and re-raises
      // them with a summary `exit 1`, so a `set +e` inside it does NOT make the cargo
      // runs non-gating. The `ci-keep-going:` comment is an explicit contract on the
      // run body (see the domain-unit job); without it, `set +e` stays disqualifying.
      const keepGoing = /ci-keep-going:/.test(script);
      let terminated = false;
      let errexitDisabled = false;
      for (const surface of shellCommandTokens(script)) {
        if (terminated) break;
        if (surface.malformed) {
          commands.push({ job: job.name, step, tokens: surface.tokens, malformed: true, controlFlow: false, gating, shell });
          continue;
        }
        const controlFlow = surface.tokens.some((token) => [";", "|", "&", "||", "&&"].includes(token));
        for (const { tokens, conditional } of commandSegments(surface)) {
          const executable = directExecutable(tokens);
          // `set -e`/`set +e` on a conditional branch (`false && set -e`) never runs, so
          // it must not change the errexit state that governs later commands.
          if (executable.tokens[0] === "set" && !conditional) {
            if (executable.tokens.some((token) => token === "+e" || /^\+[A-Za-z]*e/.test(token))) {
              errexitDisabled = true;
            }
            if (executable.tokens.some((token) => token === "-e" || /^-[A-Za-z]*e/.test(token))) {
              errexitDisabled = false;
            }
          }
          commands.push({
            job: job.name,
            step,
            tokens,
            malformed: false,
            controlFlow: controlFlow || (errexitDisabled && !keepGoing),
            gating,
            shell,
          });
          // Only an unconditional exit/return terminates the script; `false && exit 0`
          // or `cmd || exit 1` does not, so later commands must still be scanned.
          if ((executable.tokens[0] === "exit" || executable.tokens[0] === "return") && !conditional) {
            terminated = true;
            break;
          }
        }
      }
    }
  }
  return commands;
}

/** Commands from workflow steps whose result can make the workflow fail. */
export function executableWorkflowCommands(workflow) {
  return collectWorkflowCommands(workflow, false);
}

/**
 * Every run-step command, including steps a literal `if: false` or
 * `continue-on-error: true` would drop. Each entry carries `gating` (whether the step
 * can make the workflow fail) and `shell` (the effective shell override, or null for
 * the runner default) so callers can refuse protected executors that would not gate.
 */
export function allWorkflowCommands(workflow) {
  return collectWorkflowCommands(workflow, true);
}

const assignment = /^[A-Za-z_][A-Za-z0-9_]*=/;

/** Strip only transparent `command`/`env` prefixes; never unwrap echo/bash/etc. */
export function directExecutable(tokens, depth = 0) {
  if (depth > 12 || tokens.length > 512) return { tokens: [], malformed: true };
  let index = 0;
  while (assignment.test(tokens[index] ?? "")) index += 1;
  if (index >= tokens.length) return { tokens: [], malformed: false };

  if (tokens[index] === "command") {
    index += 1;
    while (tokens[index]?.startsWith("-")) {
      const option = tokens[index];
      if (option === "--") { index += 1; break; }
      // `-p` only selects a default PATH; it takes no operand, so consume it and keep
      // classifying the command that follows (`command -p timeout ...` still runs timeout).
      if (option === "-p") { index += 1; continue; }
      if (option === "-v" || option === "-V") return { tokens: [], malformed: false };
      return { tokens: [], malformed: true };
    }
    return directExecutable(tokens.slice(index), depth + 1);
  }

  if (tokens[index] === "env") {
    index += 1;
    while (tokens[index]) {
      const option = tokens[index];
      if (option === "--") { index += 1; break; }
      if (["-u", "--unset", "-C", "--chdir"].includes(option)) {
        if (!tokens[index + 1]) return { tokens: [], malformed: true };
        index += 2;
      } else if (["-S", "--split-string"].includes(option)) {
        // `env -S "timeout 30 cosign ..."` splits the operand into the real command;
        // classify that command, not `env`.
        if (!tokens[index + 1]) return { tokens: [], malformed: true };
        const surfaces = shellCommandTokens(tokens[index + 1]);
        if (surfaces.length === 1 && !surfaces[0].malformed && surfaces[0].tokens.length > 0) {
          return directExecutable(surfaces[0].tokens, depth + 1);
        }
        return { tokens: [], malformed: true };
      } else if (["-i", "--ignore-environment", "-v", "--debug"].includes(option)) {
        index += 1;
      } else if (assignment.test(option)) {
        index += 1;
      } else if (option.startsWith("-")) {
        return { tokens: [], malformed: true };
      } else {
        break;
      }
    }
    return directExecutable(tokens.slice(index), depth + 1);
  }

  return { tokens: tokens.slice(index), malformed: false };
}
