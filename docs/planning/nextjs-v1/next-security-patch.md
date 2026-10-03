# Next.js security patch — R1

Base: `f5a1141724d1fcef3609ed6125db3072c070ce2c`.
Root is the sole writer in the isolated `fix/next-security-patch` worktree.

## Outcome and evidence

Replace Next.js 16.3.5 with the compatible maintained security patch 16.3.8,
and pin its existing companion eslint-config-next to exactly 16.3.8. Preserve
React 19.3.0, Node 24.21.0, configuration, all application behavior and tests.
The approved v1 plan permits a reviewed replacement for a security advisory;
do not edit the historical approved plan or its hash.

The public GitHub advisory GHSA-vcvr-r3jv-pc5j reports critical Node next/og
ImageResponse RCE for attacker-controlled SVG content, attributes or styles,
affecting >=16.2.0,<16.3.6. The unchanged production npm audit exits 1 with
that one critical finding. Source search finds no next/og or ImageResponse
usage, so actual exploitation of this application is not established.
The official stable v16.3.8 release adds image optimization, cache and
development-server security fixes; use it rather than stopping at 16.3.6.
Public advisory/release URLs:

- https://github.com/advisories/GHSA-vcvr-r3jv-pc5j
- https://github.com/vercel/next.js/releases/tag/v16.3.8

## Compatibility and implementation

Use npm's targeted lockfile update, never audit fix --force. Inspect every
changed locked package, integrity and dependency edge. Only the necessary
Next/SWC/eslint companion closure may change; unexpected unrelated churn
returns to review. No new dependency, database migration, API, backend owner,
route or runtime setting. Read relevant newly installed Next guides before
any product code. README describes the actual new pinned version.

The public production surface remains only the root and storefront pages and
media route. Production excludes all seventeen seeded prototype business
pages and chunks. There is no schema or stored-data transition; install/build
from a single lockfile, never mix output from different Next versions. A
source rollback to the prior lock restores the known advisory and is not a
release-safe security remedy. No deployment or exposure is authorized here.

## Existing regression contract and admission

Reuse existing executable checks; a dependency-only patch does not need a
new test that duplicates a version string or upstream security implementation.
Before mutation, freeze the probe and existing test bytes, independently
review them, and commit this design. The named RED probe is:

`fnm exec --using 24.21.0 npm audit --omit=dev --audit-level=high --json`

It must execute successfully as an audit, exit 1 with the reported Next
finding on the clean approved base, and exit 0 after the patch. A network,
registry, parser or missing-runtime error cannot count as RED or GREEN.
Retain both reports, exact revisions and command results. Admit the same
probe through `backend-fork/tools/lanes/fanout.py admit` before implementation.

Required unchanged gates: npm ci; npm run lint; npm test; npm run build;
npm run test:production-routes; npm run test:e2e under Node 24.21.0, with an
isolated browser/dev-server port. Record discovery/execution/failure/skip
counts honestly. E2E prototype results do not establish live business
workflows. Audit dev dependencies separately and report unresolved findings;
do not repair unrelated tooling by automatic major upgrades. Hosted required
source-and-inquiry CI remains unchanged and must pass on the protected path.

## Boundaries and stop conditions

Allowed: package.json, package-lock.json, README.md, this design and its
review/evidence records. Forbidden: application/tests/configuration/backend,
historical plans/migrations, shared dirty roots, original Console, secrets,
live services and browser-session implementation. Root serializes lockfile
and evidence writes; reviewers are read-only. Pre-mortem: broad npm resolution
can silently upgrade unrelated packages; green prototype tests can conceal
production exposure; stale build output or audit transport failures can
produce false proof. Detect with exact lock diff, clean install/build, public
manifest/chunk/404 gate and explicit audit results. Stop on required failure,
unexpected closure changes, weakened tests or material compatibility change.

Follow the inherited frontend-shipping review sequence: four independent
design rounds and explicit exact-hash approval; approve the unchanged probes
before their implementation admission; coverage/security/simplification
review and sixteen-lens audit; exact-head independent COMMENT, protected
main merge queue while it remains the hosted default, and hosted readback.
Selected lenses: Red Team, Operability, Blast radius, Zero trust, Essentialism.
No approval here clears browser-session, identity, two-site durability,
HR/Org/Payroll/Foundry completeness, MVP or launch HOLDs.
