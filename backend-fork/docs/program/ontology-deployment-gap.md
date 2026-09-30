# The ontology authoring surface is not deployable — and a rehearsal that mostly measured its own bug

**Status:** escalation, plus a retraction. Nothing here is fixed by the lifecycle-route change that
ships alongside it.

## What is actually true

Three things, each verified by reading the tree rather than by rehearsal:

1. **No real overlay enables the command database.** `deploy/apps/console/overlays/{prod,on-prem,oci-guest}/kustomization.yaml`
   reference `components/governed-command-database` **zero** times; only the two
   `pr-473-expand-*` overlays do.
2. **Production promotion is explicitly unauthorized.**
   `docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json` carries
   `desired_state_authority_cutover`, `deployment_authorized` and `command_only` all false,
   enforced in code at `scripts/check-production-promotion-authority.py:557-561`.
3. **Without that component the api container refuses to start** — it does not degrade.
   `backend/app/src/lib.rs:705-711` makes `ONTOLOGY_COMMAND_DATABASE_URL` a hard `AppError::Config`
   for `AppRole::Api` whenever `DATABASE_URL` is set, and `deploy/apps/console/base/backend.yaml:48-49`
   sets that role. The `DatabaseDependency::NotConfigured` arm at `:2925-2930`, which would have
   produced a 503, is unreachable in that configuration. Any claim that "prod 503s" is wrong.

So the authoring surface is undeployable **by configuration and by policy**, not because the
component is defective.

## The retraction

An earlier version of this document claimed the component was missing two roles, that CNPG could not
reconcile `console_app`, and that migrations therefore died at 0112 and 0165. **All three were
artifacts of the rehearsal, not properties of the repository.**

The component declares **seven** managed roles. `console_leave_definer` and `console_ontology_writer`
are the last two (`kustomization.yaml:81-100`), carrying `login: false` and `disablePassword: true`
because they exist to be granted, never to log in. The extraction used to build the rehearsal
manifest matched on `passwordSecret:` — and those two roles are precisely the ones without it. The
regex silently dropped exactly the two records the question was about, the hand-built manifest
inherited the omission, and the cluster then faithfully reported the consequences of a gap that
existed only in the manifest.

For completeness, the same applies to the 0112 failure: per-role `ALTER ROLE … SET` defaults are
established by the component's own Job, which has each role set its own (`current_user`,
`database-topology-job.yaml:176-178`) and therefore needs no superuser. The rehearsal manifest did
not include the Job either.

`scripts/check-command-database-wiring.test.mjs` already asserts the full seven-role list, so this
was never unguarded. Running that check is what exposed the error — it failed with the two roles
appearing twice, which is only possible if they were already there.

## The lesson, which is the durable part

The rehearsal was sound in method and wrong in input. Every observation it produced was real; the
system under test was a manifest that did not match the repository. A derived artefact — a manifest,
a fixture, an extracted list — is a claim about the source, and it needs checking against the source
before conclusions are drawn from it. The specific failure mode was a filter that dropped records
silently: it returned five roles where the file had seven, and reported no error, because "no
`passwordSecret` field" is indistinguishable from "no such role" to a regex written that way.

This is the same shape as the other measurement failures recorded in the ledger — a port-forward
answering from a different database, a probe that passed on a known-bad input, a contended test run
reporting a false failure. In each, the instrument was wrong and the system was fine, and in each the
error was caught only by an independent check that had no reason to agree.

## What a real rehearsal would need

If the deployment path is to be exercised, the manifest must be produced **by kustomize from the
component**, not transcribed. `kustomize build deploy/apps/console/overlays/pr-473-expand-on-prem`
renders the intended topology, and the Job must be applied along with the Cluster — the component
ships both, in sync-waves, and testing either alone tests something the deployment never does.

The genuine open questions are unchanged and none of them is about roles: whether the ExternalSecrets
resolve against a ClusterSecretStore named `openbao-console` that exists in no cluster observed so
far, and whether production promotion should be authorized at all — which is a signature, not an
engineering decision.
