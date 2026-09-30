# A database credential must not reach a process argument list. Guard only — no
# shebang, because this file is BOTH sourced by tools/lanes/pgtest.sh and exec'd
# directly by scripts/check-test-credentials.mjs. It exists as its own file so
# that proving it does not require a bypass env var inside the harness: the
# previous shape had pgtest.sh honour CONSOLE_PGTEST_CHECK_ARGV_ONLY and `exit 0`
# without running anything, which is one line in a job's `env:` away from turning
# every PostgreSQL lane green having executed nothing.
#
# It must therefore fall off the end rather than `exit 0`: an `exit` in a sourced
# file exits pgtest.sh, and pgtest.sh has a container to start afterwards.
#
# THREAT. argv is readable by every other process on the host —
# /proc/<pid>/cmdline on Linux, `ps -ww` on macOS — for as long as the test runs,
# and CI runners are shared. GitHub Actions masks only REGISTERED secrets, so a
# URL assembled inline in a `run:` step is masked nowhere and lands in the log
# verbatim if anything echoes the command. pgtest.sh exports DATABASE_URL into
# the child's ENVIRONMENT, so a credential in argv is never necessary there and
# is therefore always a leak.
#
# SCOPE, stated rather than implied. Under Buck2 the equivalent control was
# STRUCTURAL: tools/buck/test_needs_postgres.sh:26 exits 2 on a raw //backend/...
# target, so a PostgreSQL test could not run except through a wrapper that passed
# the credential as a mode-0600 file path. This one is OPT-IN — nothing forces a
# `cargo test` through this harness, and a workflow step that talks to a database
# directly never reaches this file. Its static half (a credential on a test-runner
# line in any workflow) is what covers the invocations that skip the harness, and
# neither half covers a credential a test constructs at runtime.
#
# The three password forms a libpq client accepts are all refused:
#   postgres://user:pw@host/db          URI userinfo
#   host=... password=pw                keyword/value DSN
#   postgres://user@host/db?password=pw URI query parameter
# The second and third are why the match is case-insensitive on `password=`
# rather than the uppercase `*PASSWORD=` env-var spelling alone. Written as
# bracket classes because `${var,,}` and `shopt -s nocasematch` are bash-4-only
# and the macOS system bash is 3.2.
_console_password_assignment_re='[Pp][Aa][Ss][Ss][Ww][Oo][Rr][Dd][[:space:]]*=[[:space:]]*[^[:space:]]'
for _console_argv_guard_arg in "$@"; do
  if [[ "$_console_argv_guard_arg" =~ ://[^/@[:space:]]*:[^/@[:space:]]+@ ]] \
     || [[ "$_console_argv_guard_arg" =~ $_console_password_assignment_re ]]; then
    echo "pgtest: a database credential must not reach argv; export DATABASE_URL into the child environment instead" >&2
    exit 2
  fi
done
unset _console_argv_guard_arg _console_password_assignment_re
true
