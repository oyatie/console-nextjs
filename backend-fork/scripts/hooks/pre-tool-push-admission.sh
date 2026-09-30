#!/usr/bin/env bash
# Grok PreToolUse: deny git push / gh pr create when local admission fails.
# stdin: JSON with toolInput.command (or similar). Fail-open unless we emit deny.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INPUT="$(cat || true)"

# Extract command string loosely (jq optional)
CMD=""
if command -v jq >/dev/null 2>&1; then
  CMD="$(printf '%s' "$INPUT" | jq -r '
    .toolInput.command // .tool_input.command // .command // empty
  ' 2>/dev/null || true)"
else
  CMD="$(printf '%s' "$INPUT" | sed -n 's/.*"command"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
fi

# Only gate push / PR create / force-with-lease
if ! printf '%s' "$CMD" | grep -Eq 'git[[:space:]]+push|gh[[:space:]]+pr[[:space:]]+create|gh[[:space:]]+pr[[:space:]]+edit'; then
  exit 0
fi

if [[ "${SKIP_LOCAL_ADMISSION:-}" == "1" ]]; then
  exit 0
fi

cd "$ROOT"
if ! node scripts/local-admission.mjs >/tmp/console-admit.out 2>/tmp/console-admit.err; then
  REASON="$(head -c 1500 /tmp/console-admit.err; echo; head -c 500 /tmp/console-admit.out)"
  # JSON deny for PreToolUse
  node -e '
    const reason = process.argv[1] || "local admission failed";
    process.stdout.write(JSON.stringify({
      decision: "deny",
      reason: "ops.skip-admit: run npm run admit before push/PR. " + reason.slice(0, 1200)
    }));
  ' "$REASON"
  exit 0
fi
exit 0
