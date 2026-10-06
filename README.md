# Next.js v1 workspace

Implementation is incomplete. The [checkpoint](docs/planning/nextjs-v1/IMPLEMENTATION.md) records passing checks, the repaired clean-install gate, and the work still required before launch. The frontend still contains the inherited seeded prototype; its browser store is not a business authority.

The [device-login revocation proof](docs/planning/nextjs-v1/device-login-source-revocation-evidence.json) records the bounded repair which rejects a QR login after its recorded approving passkey is removed. Its 55 scoped PostgreSQL/library tests and two strict Clippy checks pass. The [R7 browser-session proof](docs/planning/nextjs-v1/browser-business-session-r7-evidence.json) now covers passkey sign-in, authorized own-attendance SSR/pagination/reload and exact-family logout: 121 native tests, including all 17 production-browser scenarios, pass locally. The original hosted P14 failure remains preserved with an unknown cause; the diagnostic-only revision passed all hosted checks. Two independently demonstrated test timing assumptions are now repaired, and fresh local121/17 acceptance passes. Current-revision hosted checks and protected merge-queue admission remain pending. Broader authentication, complete business workflows and launch acceptance remain on HOLD.

Use Node **24.21.0**. The locked frontend runs Next.js **16.3.8** and React **19.3.0** as a server:

```sh
npm ci
npm run lint
npm test
npm run build
npm run test:e2e
npm start
```

`npm start` serves port 5173 through the checked-in Node socket-owning server. Docker and browser checks use the same `tools/production-runtime.mjs` packager with normal `.next` output and an immutable Node 24 image. Staging runs locked `npm ci --omit=dev --omit=optional --ignore-scripts`; source builds/tests retain full `npm ci`. This prevents Next's optional Playwright peer from entering deployment. It no longer serves `out/`.

The bounded `/login/` → `/me/<context>/attendance/` journey uses an existing discoverable passkey and native own-history owner. Configure canonical `CONSOLE_PUBLIC_ORIGIN`, `CONSOLE_BACKEND_ORIGIN`, independent 32-byte unpadded base64url `CONSOLE_BROWSER_PREAUTH_KEY` and `CONSOLE_BROWSER_INGRESS_KEY`. Native also needs its separate `CONSOLE_BROWSER_SESSION_KEY_HEX` storage key and matching ingress key. Configure one trusted BFF hop with only the actual Next egress addresses. These secrets stay server-side. Public HTTPS terminates directly in `server.mjs` using mounted `CONSOLE_TLS_CERT_FILE`/`CONSOLE_TLS_KEY_FILE`; only explicit actual-loopback tests may set `CONSOLE_BROWSER_ALLOW_LOOPBACK_HTTP=true`. Generic proxies and `next start` cannot supply authenticated browser ingress. Missing browser configuration leaves storefront available and browser access closed.

Each sign-in has a separate HttpOnly context cookie with its original fixed deadline. Reads never rotate tokens. Logout clears only that cookie after native confirmation; missing/expired access is not a logout receipt. Restored attendance documents hide private content and reload for fresh authorization. This source slice does not authorize deployment or clear the full HR/Org/Payroll/Foundry, two-site durability, physical-passkey or launch HOLDs.

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
