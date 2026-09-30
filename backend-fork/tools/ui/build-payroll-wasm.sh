#!/usr/bin/env bash
# Rebuild committed GET /_ui/pkg bindgen output from console-payroll-ui (hydrate+islands).
# CLI wasm-bindgen must match Cargo.toml (0.2.123). Do not commit debug wasm.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root/backend"
cargo build -p console-payroll-ui --target wasm32-unknown-unknown \
  --no-default-features --features hydrate,islands --lib --release
mkdir -p crates/payroll/ui/pkg
wasm-bindgen target/wasm32-unknown-unknown/release/console_payroll_ui.wasm \
  --out-dir crates/payroll/ui/pkg --out-name console_payroll_ui --target web
rm -f crates/payroll/ui/pkg/*.d.ts
