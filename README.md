# Next.js v1 workspace

Implementation is incomplete. The [checkpoint](docs/planning/nextjs-v1/IMPLEMENTATION.md) records passing checks, the repaired clean-install gate, and the work still required before launch. The frontend still contains the inherited seeded prototype; its browser store is not a business authority.

The [device-login revocation proof](docs/planning/nextjs-v1/device-login-source-revocation-evidence.json) records the bounded repair which rejects a QR login after its recorded approving passkey is removed. Its 55 scoped PostgreSQL/library tests and two strict Clippy checks pass; broader authentication, Next business sessions and launch acceptance remain on HOLD.

Use Node **24.21.0**. The locked frontend runs Next.js **16.3.5** and React **19.3.0** as a server:

```sh
npm ci
npm run lint
npm test
npm run build
npm run test:e2e
npm start
```

`npm start` serves port 5173. Docker uses the standalone server output and an immutable Node 24 image. It no longer serves `out/`; the old output directory is excluded from builds.

The public `/storefront` pages require `CONSOLE_BACKEND_ORIGIN` at runtime, set to the Rust server origin (`https://host[:port]`, without a path or credentials). Next fetches the live public catalog and submits inquiries server-side. A missing or unavailable backend shows an error; no fixture listings are served.

Before public exposure, preserve a validated client IP through a trusted ingress: server-side inquiry submissions otherwise share Rust's five-per-minute IP limit at the Next host. Rust has no stable inquiry receipt lookup, so the UI confirms only local receipt and treats lost responses as uncertain. A selected listing that becomes unavailable returns a generic conflict without recording an inquiry; the form preserves input and requires an explicit switch to a general inquiry.

Public listing images use a signed, size-limited S3 read through Rust and an uncached Next proxy. A governed listing-photo upload/publish owner and real private-store/browser/withdrawal proof are still required before this path is exposed publicly. The storage adapter remains an interoperability path; canonical PostgreSQL file custody and two-site confirmation from the plan are not implemented for these images.

`backend-fork/` is an independent copy of the clean Rust source at `b5fc06e335795231159c7c61cee0d39d0bb54b16`. Its manifest records the original source HEAD, staged tree, working-file hashes, and copy verification. No source-repository writes, deployments, or live institutional operations were performed. Copied upstream deployment manifests are reference/test inputs, not authorization to apply them.

The initial acceptance runner records commands, input/output hashes, and test counts under `.artifacts/acceptance/`. It currently exits nonzero because required launch acceptance remains unmet:

```sh
npm run test:source-closure
npm run test:acceptance -- --baseline
# After building the locked Rust workspace, use a local PostgreSQL 18 installation:
npm run test:acceptance -- --cargo --postgres-bin /path/to/postgresql18/bin
```

The PostgreSQL probe creates and removes its own socket-only cluster, uses the real migration owner, and never reads live database credentials. Run Rust builds from `backend-fork/backend/` so its pinned Rust toolchain applies; set `SQLX_OFFLINE=true`. The probe expects the application at `.artifacts/cargo-target/debug/console-app`, which can be built with `cargo build --locked --offline -p console-app --target-dir ../../.artifacts/cargo-target` from that directory.

Fresh databases use the schema baseline at historical version 225; existing databases retain their original SQLx migration history. Do not run the old migrator on a fresh baseline or remove its version-0 record. The [transition record](docs/planning/nextjs-v1/empty-install-transition.md) documents compatibility and rollback. Reproduce the focused gate with `python3 tools/check-empty-install.py --postgres-bin /path/to/postgresql18/bin`; add `--legacy-app /path/to/frozen-console-app` for actual old-runner refusal proof. `--generate-baseline` regenerates only from the frozen historical SQL in a disposable cluster and does not assert acceptance.
