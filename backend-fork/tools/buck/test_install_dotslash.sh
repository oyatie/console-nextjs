#!/usr/bin/env bash
# Static regression lock for the immutable DotSlash bootstrap manifest.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
installer="${repo_root}/tools/buck/install_dotslash.sh"
bash -n "${installer}"
grep -Fq 'version="v0.5.7"' "${installer}"
grep -Fq 'dotslash-ubuntu-22.04.x86_64.v0.5.7.tar.gz' "${installer}"
grep -Fq '34bf598e229962cedd96999e40c7396152de68a43a04acae0e2fb3095ba81e92' "${installer}"
grep -Fq 'dotslash-ubuntu-22.04.arm64.v0.5.7.tar.gz' "${installer}"
grep -Fq '8f3c94a4adf68b6042d93c700d835744d481c8cc5d85f0eb36c5e22119533e02' "${installer}"
grep -Fq 'dotslash-macos.v0.5.7.tar.gz' "${installer}"
grep -Fq 'e6048c4f307469799ecfd232588616275bb291519117cf30e158341ca5252827' "${installer}"
grep -Fq -- '--proto' "${installer}"
grep -Fq 'shasum -a 256 --check --status' "${installer}"
grep -Fq 'DotSlash ${version#v}' "${installer}"
echo "install-dotslash: contract PASS"
