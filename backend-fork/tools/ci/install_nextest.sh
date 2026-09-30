#!/usr/bin/env bash
# Install the pinned cargo-nextest from its published release archive.
#
# WHY A HASH-PINNED TARBALL AND NOT `cargo install`.
# `cargo install cargo-nextest --locked --version 0.9.138` compiles it from
# source, which costs minutes on every shard that needs it and eats the win the
# runner exists to deliver (measured 2.35x on the heaviest PostgreSQL target:
# identity-rest-org-setup-pg, 43.472s under cargo vs 18.525s under nextest, with
# 39/39 tests identical). The prebuilt archive installs in seconds.
#
# WHY THIS IS NOT A NEW SUPPLY-CHAIN POSTURE.
# It is the shape `security.yml` already uses for Trivy, pinned byte-for-byte by
# `scripts/check-workflow-hardening.mjs`: fetch one exact URL, verify one exact
# sha256 BEFORE extracting, extract one named member. kubectl in the same file
# uses the same curl + `sha256sum --check` pattern. Nothing here is trusted that
# the hash does not cover.
#
# The version is the one `.config/nextest.toml` already pins. That file's comment
# named `taiki-e/install-action`, which this repository does not use and has never
# used; this script is what actually implements the pin.
set -euo pipefail

NEXTEST_VERSION="0.9.138"
NEXTEST_SHA256="3793bf0c27607b196f502c39b2108f571de89fcda7586ae6beefa11ee177b216"
# Written out in full, not assembled from ${NEXTEST_VERSION}. An interpolated URL
# cannot be grepped, so `scripts/check-ci-preflight.mjs` could not pin it and a
# reviewer could not read what is actually fetched. This mirrors the Trivy install
# in security.yml, whose literal URL is pinned the same way.
NEXTEST_URL="https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.138/cargo-nextest-0.9.138-x86_64-unknown-linux-gnu.tar.gz"

install_root="${1:-/usr/local/bin}"
tmp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
archive="${tmp}/cargo-nextest-${NEXTEST_VERSION}.tar.gz"
extract_dir="${tmp}/cargo-nextest-install"

curl --disable --fail --silent --show-error --location \
  "${NEXTEST_URL}" \
  --output "${archive}"

# Verified BEFORE extraction, never after: a tarball that fails the check is
# never unpacked, so nothing it contains can run.
printf '%s  %s\n' "${NEXTEST_SHA256}" "${archive}" | sha256sum --check

mkdir -p "${extract_dir}"
tar -xzf "${archive}" -C "${extract_dir}" cargo-nextest
# The destination is created here, not assumed. CI passes
# "${RUNNER_TEMP}/nextest-bin", which does not exist on a fresh runner, and
# `install` does not create intermediate directories -- the first CI run failed
# exactly here. The local container test had passed only because the harness
# ran `mkdir -p` on the target first, which is the harness hiding the defect
# rather than the script working.
mkdir -p "${install_root}"
install -m 0755 "${extract_dir}/cargo-nextest" "${install_root}/cargo-nextest"

# Prove the thing that was installed is the thing that was pinned. A silent
# no-op installer is worse than none: the shard would fall back to cargo and
# report a green run that measured nothing.
installed_version="$("${install_root}/cargo-nextest" --version)"
case "${installed_version}" in
  *"${NEXTEST_VERSION}"*) ;;
  *)
    echo "install_nextest: expected ${NEXTEST_VERSION}, got '${installed_version}'" >&2
    exit 1
    ;;
esac
echo "install_nextest: ${installed_version}"
