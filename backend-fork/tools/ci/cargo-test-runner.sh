#!/usr/bin/env bash
# Run a stream of cargo test invocations (JSONL on stdin) with fail-slow
# keep-going semantics. Each stdin line is one JSON object:
#   {"name": "...", "package": "...", "argv": ["cargo", "test", ...]}
#
# Extracted from cargo_needs_postgres.sh so the keep-going loop is unit-testable
# without Docker or a real Cargo workspace: feed it JSONL and point PATH at a
# stubbed `cargo`.
#
# Exit 0 only when every invocation passed; exit 1 when any failed (the workflow
# relies on non-zero == red). Keep-going is the default; --fail-fast aborts after
# the first failure (local use). `set -e` is deliberately OUT of the per-binary
# loop: failures are captured explicitly so one red binary never hides the rest.
#
# Environment (set by the caller):
#   CARGO_REPO_ROOT  repository root used as the cargo cwd
#   RUST_TEST_THREADS number of test threads per invocation (default 1)
set -uo pipefail

repo_root="${CARGO_REPO_ROOT:-}"
threads="${RUST_TEST_THREADS:-1}"
fail_fast=0
case "${1:-}" in
  --keep-going) fail_fast=0 ;;
  --fail-fast) fail_fast=1 ;;
  "")
    ;;
  *)
    echo "usage: cargo-test-runner.sh [--keep-going|--fail-fast]" >&2
    exit 2
    ;;
esac

if [[ -z "${repo_root}" ]]; then
  echo "cargo-test-runner: CARGO_REPO_ROOT must name the repository root" >&2
  exit 2
fi

passed=0
failed=0
results=()

while IFS= read -r row || [[ -n "${row}" ]]; do
  name="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["name"])' "${row}")"
  echo "cargo-postgres: === ${name} ==="
  # JSON array exec avoids the word-splitting bugs of `set -- $(...)`; the argv
  # is taken from the map entry verbatim.
  if python3 - "${repo_root}" "${row}" "${threads}" <<'PY'
import json, os, subprocess, sys, time
root, row, threads = sys.argv[1], json.loads(sys.argv[2]), sys.argv[3]
argv = row["argv"]
env = os.environ.copy()
env["SQLX_OFFLINE"] = "true"
env["RUST_TEST_THREADS"] = threads
env["CARGO_TERM_COLOR"] = "always"
print("cargo-postgres:", " ".join(argv), flush=True)
started = time.monotonic()
code = subprocess.call(argv, cwd=root, env=env)
elapsed = time.monotonic() - started
# One machine-readable line per invocation, on stdout, unconditionally.
#
# Shard balance is currently decided by *entry count* (postgres-shard.mjs
# greedy-packs by how many targets a package owns), which assumes every target
# costs the same. Measured 2026-08-18 they do not: the five shards ran
# 1407/1094/1062/974/845s, so the slowest shard alone was 92% of a 1534s run.
# Rebalancing needs per-target durations, and this is the only place that knows
# them.
#
# Deliberately stdout, not an artifact: an upload step would have to be added to
# five jobs, each of which has its ordered step list locked by the CI preflight
# mirror. A log line needs no workflow change at all, so this cannot desync from
# the workflow contract. `status` is recorded because a failed invocation's
# duration is not comparable to a passing one and must not be packed on.
print(
    "cargo-postgres-timing: "
    + json.dumps(
        {
            "name": row.get("name"),
            "package": row.get("package"),
            "seconds": round(elapsed, 3),
            "status": "pass" if code == 0 else "fail",
            "shard": os.environ.get("CARGO_POSTGRES_SHARD_ID", ""),
        },
        sort_keys=True,
    ),
    flush=True,
)
raise SystemExit(code)
PY
  then
    passed=$((passed + 1))
    results+=("PASS  ${name}")
  else
    failed=$((failed + 1))
    results+=("FAIL  ${name}")
    if [[ "${fail_fast}" == 1 ]]; then
      break
    fi
  fi
done

echo ""
echo "cargo-postgres: summary"
for entry in "${results[@]}"; do
  printf '  %s\n' "${entry}"
done
echo "cargo-postgres: ${passed} passed, ${failed} failed"

if [[ "${failed}" != 0 ]]; then
  echo "cargo-postgres: one or more invocations failed" >&2
  exit 1
fi
echo "cargo-postgres: all ${passed} invocations passed"
