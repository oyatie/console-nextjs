#!/usr/bin/env python3
"""Behavior locks for the repository-owned Reindeer bootstrap and closures."""

import tomllib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class ReindeerBootstrapTests(unittest.TestCase):
    def read(self, relative_path: str) -> str:
        return (ROOT / relative_path).read_text()

    def test_reindeer_bootstrap_is_pinned_and_does_not_fall_back_to_path(self) -> None:
        lock = self.read("third-party/rust/reindeer/upstream.lock")
        bootstrap = self.read("third-party/rust/reindeer/bootstrap.sh")

        self.assertIn("REINDEER_COMMIT=681727ced54a853977ac495e147ac54e1c0db115", lock)
        self.assertIn(
            "REINDEER_ARCHIVE_SHA256=79c09407900ed8d2b70b620af24cb2e2c6b0661d75804326ae1ee77fe908259a",
            lock,
        )
        self.assertIn("REINDEER_TOOLCHAIN=nightly-2026-02-28", lock)
        self.assertIn("--fuzz=0", bootstrap)
        self.assertNotIn("command -v reindeer", bootstrap)

    def test_workspace_dev_closure_is_generated_from_direct_roots(self) -> None:
        config = self.read("third-party/rust/reindeer.toml")
        patch = self.read(
            "third-party/rust/reindeer/patches/0001-workspace-dev-dependency-roots.patch"
        )
        graph = self.read("third-party/rust/BUCK")

        self.assertIn("include_workspace_dev_dependencies = true", config)
        self.assertIn("workspace_dev_dependency_roots", patch)
        self.assertIn("DepKind::Dev", patch)
        self.assertIn("TargetReq::Lib", patch)
        self.assertIn('name = "webauthn-authenticator-rs"', graph)
        self.assertIn('name = "tempfile"', graph)

    def test_reindeer_bootstrap_uses_security_fixed_cargo_graph(self) -> None:
        bootstrap = self.read("third-party/rust/reindeer/bootstrap.sh")
        cargo_patch = self.read(
            "third-party/rust/reindeer/patches/0002-cargo-security-uplift.patch"
        )
        cargo_lock = self.read("third-party/rust/reindeer/Cargo.lock")

        self.assertIn('cargo = "0.98"', cargo_patch)
        self.assertIn("0002-cargo-security-uplift.patch", bootstrap)
        self.assertIn('name = "cargo"\nversion = "0.98.0"', cargo_lock)
        self.assertIn('name = "gix"\nversion = "0.83.0"', cargo_lock)
        self.assertIn('name = "gix-fs"\nversion = "0.21.2"', cargo_lock)
        self.assertIn('name = "gix-pack"\nversion = "0.70.0"', cargo_lock)
        self.assertIn('name = "gix-validate"\nversion = "0.11.3"', cargo_lock)

    def test_openssl_product_closure_is_vendored_and_uses_supported_cfgs(self) -> None:
        auth_manifest = self.read("backend/crates/platform/auth/Cargo.toml")
        openssl_sys = self.read("third-party/rust/fixups/openssl-sys/fixups.toml")
        openssl = self.read("third-party/rust/fixups/openssl/fixups.toml")
        graph = self.read("third-party/rust/BUCK")

        self.assertIn('openssl = { version = "0.10.80", features = ["vendored"] }', auth_manifest)
        self.assertEqual(
            tomllib.loads(openssl_sys)["buildscript"]["run"],
            {
                "env": {
                    "CARGO_MAKEFLAGS": "-j4",
                    "OPENSSL_RUST_USE_NASM": "0",
                },
                "rustc_link_lib": True,
                "rustc_link_search": True,
            },
        )
        openssl_sys_run = graph.split(
            'name = "openssl-sys-0.9-build-script-main-run",', 1
        )[1].split("    features = [", 1)[0]
        generated_env = openssl_sys_run.split("    env = ", 1)[1]
        self.assertEqual(
            generated_env,
            '{\n'
            '        "CARGO_MAKEFLAGS": "-j4",\n'
            '        "CARGO_MANIFEST_LINKS": "openssl",\n'
            '        "CARGO_PKG_AUTHORS": "Alex Crichton <alex@alexcrichton.com>:Steven Fackler <sfackler@gmail.com>",\n'
            '        "CARGO_PKG_DESCRIPTION": "FFI bindings to OpenSSL",\n'
            '        "CARGO_PKG_HOMEPAGE": "",\n'
            '        "CARGO_PKG_README": "README.md",\n'
            '        "CARGO_PKG_REPOSITORY": "https://github.com/rust-openssl/rust-openssl",\n'
            '        "CARGO_PKG_RUST_VERSION": "1.80.0",\n'
            '        "CARGO_PKG_VERSION_MAJOR": "0",\n'
            '        "CARGO_PKG_VERSION_MINOR": "9",\n'
            '        "CARGO_PKG_VERSION_PATCH": "116",\n'
            '        "CARGO_PKG_VERSION_PRE": "",\n'
            '        "OPENSSL_RUST_USE_NASM": "0",\n'
            "    },\n",
        )
        self.assertIn('name = "openssl-src-300.6.1+3.6.3.crate"', graph)
        self.assertIn('name = "openssl-src-300"', graph)
        self.assertIn('osslconf="OPENSSL_NO_IDEA"', openssl)
        self.assertIn('osslconf="OPENSSL_NO_SEED"', openssl)
        self.assertNotIn("ossl360", openssl)
        self.assertNotIn("ossl361", openssl)

    def test_ring_native_link_fixup_covers_linux_architectures(self) -> None:
        ring = self.read("third-party/rust/fixups/ring/fixups.toml")
        graph = self.read("third-party/rust/BUCK")

        self.assertIn(
            "cfg(all(target_arch = \"x86_64\", target_os = \"linux\"))",
            ring,
        )
        self.assertIn(
            "cfg(all(target_arch = \"aarch64\", target_os = \"linux\"))",
            ring,
        )
        expected_x86_64_sources = (
            "crypto/crypto.c",
            "crypto/cpu_intel.c",
            "crypto/curve25519/curve25519.c",
            "crypto/curve25519/curve25519_64_adx.c",
            "crypto/fipsmodule/aes/aes_nohw.c",
            "crypto/fipsmodule/bn/montgomery.c",
            "crypto/fipsmodule/bn/montgomery_inv.c",
            "crypto/fipsmodule/ec/ecp_nistz.c",
            "crypto/fipsmodule/ec/gfp_p256.c",
            "crypto/fipsmodule/ec/gfp_p384.c",
            "crypto/fipsmodule/ec/p256.c",
            "crypto/fipsmodule/ec/p256-nistz.c",
            "crypto/limbs/limbs.c",
            "crypto/mem.c",
            "crypto/poly1305/poly1305.c",
            "third_party/fiat/asm/fiat_curve25519_adx_mul.S",
            "third_party/fiat/asm/fiat_curve25519_adx_square.S",
            "pregenerated/aes-gcm-avx2-x86_64-elf.S",
            "pregenerated/aesni-gcm-x86_64-elf.S",
            "pregenerated/aesni-x86_64-elf.S",
            "pregenerated/chacha-x86_64-elf.S",
            "pregenerated/chacha20_poly1305_x86_64-elf.S",
            "pregenerated/ghash-x86_64-elf.S",
            "pregenerated/p256-x86_64-asm-elf.S",
            "pregenerated/x86_64-mont5-elf.S",
            "pregenerated/x86_64-mont-elf.S",
            "pregenerated/sha256-x86_64-elf.S",
            "pregenerated/sha512-x86_64-elf.S",
            "pregenerated/vpaes-x86_64-elf.S",
        )
        expected_aarch64_sources = (
            "crypto/curve25519/curve25519.c",
            "crypto/fipsmodule/aes/aes_nohw.c",
            "crypto/fipsmodule/bn/montgomery.c",
            "crypto/fipsmodule/bn/montgomery_inv.c",
            "crypto/fipsmodule/ec/ecp_nistz.c",
            "crypto/fipsmodule/ec/gfp_p256.c",
            "crypto/fipsmodule/ec/gfp_p384.c",
            "crypto/fipsmodule/ec/p256.c",
            "crypto/fipsmodule/ec/p256-nistz.c",
            "crypto/limbs/limbs.c",
            "crypto/mem.c",
            "crypto/poly1305/poly1305.c",
            "pregenerated/aesv8-armx-linux64.S",
            "pregenerated/aesv8-gcm-armv8-linux64.S",
            "pregenerated/armv8-mont-linux64.S",
            "pregenerated/chacha-armv8-linux64.S",
            "pregenerated/chacha20_poly1305_armv8-linux64.S",
            "pregenerated/ghash-neon-armv8-linux64.S",
            "pregenerated/ghashv8-armx-linux64.S",
            "pregenerated/p256-armv8-asm-linux64.S",
            "pregenerated/sha256-armv8-linux64.S",
            "pregenerated/sha512-armv8-linux64.S",
            "pregenerated/vpaes-armv8-linux64.S",
        )
        for source in expected_x86_64_sources + expected_aarch64_sources:
            self.assertIn(source, ring)

        self.assertIn('"linux-x86_64": dict(', graph)
        self.assertIn('"linux-arm64": dict(', graph)
        self.assertIn(":ring-0.17-ring-c-asm-linux-x86_64", graph)
        self.assertIn(":ring-0.17-ring-c-asm-linux-arm64", graph)
        self.assertIn('name = "ring-0.17-ring-c-asm-linux-x86_64"', graph)
        self.assertIn('name = "ring-0.17-ring-c-asm-linux-arm64"', graph)
        for target, expected_sources in (
            ("ring-0.17-ring-c-asm-linux-x86_64", expected_x86_64_sources),
            ("ring-0.17-ring-c-asm-linux-arm64", expected_aarch64_sources),
        ):
            cxx_rule = graph.split(f'name = "{target}"', 1)[1].split("headers =", 1)[0]
            for source in expected_sources:
                self.assertIn(f":ring-0.17.14.crate[{source}]", cxx_rule)


if __name__ == "__main__":
    unittest.main()
