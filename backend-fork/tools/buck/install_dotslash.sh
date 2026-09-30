#!/usr/bin/env bash
# Install the exact DotSlash runtime required by the repository-pinned Buck2
# manifest. CI must run this before invoking ./tools/buck2.
set -euo pipefail

version="v0.5.7"
case "$(uname -s):$(uname -m)" in
  Linux:x86_64)
    archive_name="dotslash-ubuntu-22.04.x86_64.v0.5.7.tar.gz"
    sha256="34bf598e229962cedd96999e40c7396152de68a43a04acae0e2fb3095ba81e92"
    ;;
  Linux:aarch64|Linux:arm64)
    archive_name="dotslash-ubuntu-22.04.arm64.v0.5.7.tar.gz"
    sha256="8f3c94a4adf68b6042d93c700d835744d481c8cc5d85f0eb36c5e22119533e02"
    ;;
  Darwin:*)
    # The release's universal macOS binary prevents an architecture mismatch
    # between the runner and nested DotSlash invocations.
    archive_name="dotslash-macos.v0.5.7.tar.gz"
    sha256="e6048c4f307469799ecfd232588616275bb291519117cf30e158341ca5252827"
    ;;
  *)
    echo "install-dotslash: unsupported platform $(uname -s):$(uname -m)" >&2
    exit 2
    ;;
esac

bin_dir="${CONSOLE_DOTSLASH_BIN_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}/console-dotslash}/bin}"
mkdir -p "${bin_dir}"
archive="$(mktemp "${TMPDIR:-/tmp}/dotslash.XXXXXX.tar.gz")"
trap 'rm -f "${archive}"' EXIT
url="https://github.com/facebook/dotslash/releases/download/${version}/${archive_name}"

curl --fail --location --proto '=https' --tlsv1.2 --retry 5 --silent --show-error \
  --output "${archive}" "${url}"
printf '%s  %s\n' "${sha256}" "${archive}" | shasum -a 256 --check --status
tar -xzf "${archive}" -C "${bin_dir}"

if [[ ! -x "${bin_dir}/dotslash" ]]; then
  echo "install-dotslash: archive did not contain an executable dotslash" >&2
  exit 1
fi
actual_version="$("${bin_dir}/dotslash" --version)"
if [[ "${actual_version}" != "DotSlash ${version#v}" ]]; then
  echo "install-dotslash: expected DotSlash ${version#v}, got ${actual_version}" >&2
  exit 1
fi
if [[ -n "${GITHUB_PATH:-}" ]]; then
  printf '%s\n' "${bin_dir}" >>"${GITHUB_PATH}"
fi
printf 'install-dotslash: %s installed at %s\n' "${actual_version}" "${bin_dir}"
