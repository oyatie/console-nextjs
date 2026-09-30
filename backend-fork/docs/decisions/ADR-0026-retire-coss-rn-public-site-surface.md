---
id: ADR-0026
status: accepted
doc_status: published
date: 2026-07-22
owner: jasonlee
decision: retire-coss-rn-public-site-surface
amends: [ADR-0023]
related: [ADR-0009, ADR-0012, ADR-0023]
---

# ADR-0026: Retire the standalone COSS RN public-site surface

## Status

**Accepted 2026-07-22.** This is an intentional breaking product-surface
retirement. It is pre-1.0 minor-release material, not a patch-level change.

## Context

`coss-rn/` shipped as a standalone public-site React Native application from
v0.1.12. It was not part of Console's native field-app parity scope:
ADR-0009 and the parity checklist explicitly separated that public-site surface
from the Swift and Kotlin technician applications.

Commit `9b053fe4` removed the source directory but left it declared as a root
npm workspace and retained its package-lock entries. That leaves fresh npm
installs and workspace-aware tooling pointing at a path that no longer exists.
It also leaves no durable decision record for consumers of the former public
site or for release reviewers deciding the required version boundary.

## Decision

This ADR amends only ADR-0023's COSS RN follow-up sentence. It does not change
ADR-0023's console authority, ADR-0009's dual-native employee-app scope, or
ADR-0012's four-deliverable monorepo boundary.

1. Retire `coss-rn/` as a shipped public product surface. It has no replacement
   route, compatibility host, or implied migration claim in this repository.
2. Remove `coss-rn` from the root npm workspace manifest and lockfile. Root
   workspace declarations must resolve to a present directory with a
   `package.json`; the automated CI gate fails closed otherwise.
3. Historical COSS RN assets, tests, hosts, and release evidence are invalid
   evidence for Console parity, current release readiness, or current
   public-site availability.
4. Publish this retirement in the next pre-1.0 **minor** release. A release
   candidate must describe the removed public-site surface and must not classify
   the change as a patch. From the current `0.1.65` baseline, `0.1.66` is
   therefore insufficient; the release line must advance to at least `0.2.0`.

## Consequences

- Consumers must stop relying on the retired COSS RN public site and obtain any
  necessary replacement product direction through a separately approved
  product decision.
- npm installation and workspace tooling become internally consistent and
  reject future orphaned workspace entries before CI can report success.
- Console's iOS/Android parity remains governed by ADR-0009 without
  inheriting COSS RN tests, strings, or screenshots as evidence.

## Alternatives considered

### Keep a placeholder `coss-rn` workspace

Rejected. A placeholder would preserve a misleading public-product and npm
contract, hide the breaking retirement, and permit stale evidence to be cited.

### Silently delete the directory and classify the release as a patch

Rejected. Deleting a shipped standalone surface is externally material even
before 1.0; semantic versioning requires a minor release and a durable record.
