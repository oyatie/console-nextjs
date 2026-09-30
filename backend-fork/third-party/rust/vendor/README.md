# Rust third-party vendor patches

Temporary crates.io patches used only when the published dependency graph cannot satisfy CI/security policy.

This directory is the single repository-owned source boundary for those
patches. Cargo references it from `backend/Cargo.toml`; Reindeer and Buck2 can
materialize it without crossing out of `third-party/rust`; and the backend
image copies the same audited bytes. Do not create a second backend-local
vendor copy.

## quick-xml RUSTSEC unblock

`calamine 0.35.0` depends on `quick-xml ^0.39` and `umya-spreadsheet 3.0.0` depends on `quick-xml ^0.37.1`. Both ranges resolve to versions below the fixed `quick-xml >=0.41.0` line for RUSTSEC-2026-0194/RUSTSEC-2026-0195.

The local patches are:

- `calamine-0.35.0-quickxml41`: published `calamine 0.35.0` with its `quick-xml` dependency raised to `0.41.0`.
- `umya-spreadsheet-3.0.0-quickxml41`: published `umya-spreadsheet 3.0.0` with its `quick-xml` dependency raised to `0.41.0`.
- `quick-xml-0.41.0-compat`: published `quick-xml 0.41.0` plus a small `BytesText::unescape` compatibility shim for `umya-spreadsheet`'s pre-0.41 API usage.

Remove these patches once upstream crates.io releases allow the workspace to resolve `quick-xml >=0.41.0` without local shims.
