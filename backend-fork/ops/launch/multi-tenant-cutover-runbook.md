# Multi-tenant cutover runbook (feat/multi-tenant-phase1 → prod)

Status: DRAFT for review. Do NOT execute until reviewed and a maintenance window
is scheduled. This cutover transforms the **live** `maintenance` database (adds
`org_id` everywhere, backfills KNL as tenant #1, enables FORCE RLS, and de-owns
the application's DB role) and switches the running app from connecting as the
table **owner** (`console_app`) to the non-owner runtime role (`console_rt`). It is
partly irreversible without a backup restore.

The single hard constraint that makes this a *windowed* cutover: once RLS is
enabled (migrations 0030/0035) with `FORCE ROW LEVEL SECURITY`, **the old app —
which connects as the owner and never sets `app.current_org` — reads zero rows**.
So the schema migration and the new (`console_rt`, GUC-setting) app must go live
together, with the old app not serving in between. A short scheduled maintenance
window is the safe, simple way to guarantee that.

---

## 0. Pre-flight (do BEFORE the window; none of this touches live data)

0. **Take the `maintenance` Argo app off auto-sync FIRST.** It is
   `automated: { prune: true, selfHeal: true }` (`deploy/argocd/apps/console.yaml`),
   so a merge to `main` would otherwise *immediately* apply the whole cutover
   (the wave-2 Sync migrate Job enables RLS on the live DB, and the app Deployment
   flips `DATABASE_URL` to `console-db-rt`) as an uncontrolled rolling update — an
   outage. Switching to manual sync is **non-disruptive**: running workloads keep
   running; Argo just stops auto-applying git. Do this before the merge:
   ```sh
   kubectl -n argocd patch application maintenance --type merge \
     -p '{"spec":{"syncPolicy":{"automated":null}}}'
   # or: argocd app set maintenance --sync-policy none
   ```
   (Re-enable auto-sync in §4 once the cutover is verified.) Note the root
   app-of-apps also self-heals; if it reverts this patch, set the override on the
   root too, or pause the root briefly.


1. **Branch is merged + images built.** `feat/multi-tenant-phase1` is merged to
   `main`, CI green; `image-release` has built+signed new `console-app` and `console-web`
   images by digest. Record the two `sha256:` digests.
2. **All non-owner login secrets exist.** Create `console-db-rt`,
   `console-db-leave-command`, and `console-db-ontology-command` per
   `deploy/SECRETS.md`, each with matching `username`, URL-safe generated
   `password`, and `uri` fields. Preserve all three in the authoritative Vault
   recovery bundle. They must exist before CNPG and the API/worker sync.
3. **Managed role config.** `deploy/.../database.yaml` declares migration-only
   `console_app` as `BYPASSRLS`, while `console_rt` and both command logins are
   `NOSUPERUSER NOBYPASSRLS`. Confirm Argo will reconcile the exact attributes
   and bind the separate password Secrets before migration 0031 runs.
4. **Backup is fresh + restore is proven.** Confirm a recent CNPG/Barman base
   backup + WAL archiving is current (`ops/dr/cnpg-restore-drill.*`). Note the
   exact pre-cutover timestamp — this is the PITR rollback target.
5. **Migrate runner ready (#14).** The `console-app` image supports `CONSOLE_APP_ROLE=
   migrate`, and the Argo wave-2 Sync `migrate-job` runs it as the OWNER
   (`console-db-app` secret) before the app syncs. Verify the Job renders
   (`kubectl kustomize deploy/apps/console/base`).
6. **Full sync only.** Do not selectively sync the migration Job, API, or
   worker. Sync the maintenance Application as a whole so CNPG, topology
   readback, migration, and serving waves execute in order.
   - **_sqlx_migrations alignment:** prod was migrated manually with sqlx-cli, so
     `_sqlx_migrations` already records 0001..0025 with checksums. The embedded
     Migrator must match those checksums byte-for-byte or it will refuse. CONFIRM
     before the window: dump prod `_sqlx_migrations` and diff the checksums for
     0001..0025 against the embedded files. If they differ (whitespace/edits),
     reconcile before cutover.
6. **Announce** the maintenance window to the operator(s).

---

## 1. Cutover (inside the maintenance window)

> All commands assume an authenticated `kubectl`/`oc` against the cluster and an
> SSH tunnel / psql access to the DB as the **owner** (`console_app`) for checks.

1. **Quiesce the app.** Scale the API + worker to 0 (or serve a maintenance page
   at the ingress). This guarantees no owner-no-GUC reads hit an RLS-enabled DB
   and no writes land mid-migration.
   ```sh
   kubectl -n console scale deploy/console-app deploy/console-worker --replicas=0
   ```
2. **Snapshot point.** Record `SELECT now();` (the PITR rollback target) and
   confirm WAL is archiving.
3. **Apply migrations 0026→0037.** Let the ordered Sync `migrate-job` run after
   the wave-1 database topology readback gate, or run the same image as `console_app` on the
   next sync (it connects as owner via `console-db-app`), or run them manually as
   owner through the tunnel:
   ```sh
   # manual fallback (owner connection)
   SQLX_OFFLINE=true DATABASE_URL="postgres://console_app:***@127.0.0.1:5432/console?sslmode=disable" \
     sqlx migrate run --source backend/crates/platform/db/migrations
   ```
   This adds `org_id` (backfilled to KNL `…00a1`), enforces NOT NULL + FKs,
   enables FORCE RLS, de-owns grants to `console_rt`, and re-homes the cold-start
   admin to the platform sentinel org (`…face`). Watch for errors; it is
   transactional per file.
4. **Verify the schema cutover (as owner, RLS now on):**
   ```sql
   -- KNL is tenant #1, sentinel exists
   SELECT id, slug, status FROM organizations ORDER BY slug;       -- knl + platform
   -- RLS forced on a representative table
   SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE relname='work_orders';  -- t,t
   -- runtime role is non-owner, non-bypass
   SELECT rolname, rolsuper, rolbypassrls FROM pg_roles WHERE rolname IN ('console_app','console_rt');
   ```
5. **Pin the new signed digests** in
   `deploy/apps/console/overlays/prod/kustomization.yaml` (`console-app` +
   `console-web`), commit to `main`. Auto-sync is OFF (§0.0), so Argo will NOT apply
   yet — it shows OutOfSync and waits. Trigger the cutover deliberately with a
   **manual sync**, which runs the wave-2 Sync `migrate-job` (migrations 0026–0037,
   incl. RLS enable) to completion first, then rolls the Deployments to the new
   digest (now connecting as `console_rt`):
   ```sh
   argocd app sync maintenance        # CNPG → topology gate → migrate → app/worker/web
   # or: kubectl -n argocd annotate application maintenance argocd.argoproj.io/sync=...
   ```
   Watch the `console-migrate` Job to Completed before the Deployments roll.
6. **Scale the app back up** (Argo will, as it deploys the new Deployment which
   now reads `DATABASE_URL` from `console-db-rt`). The new app connects as the
   non-owner role and sets `app.current_org` per request, so RLS resolves to the
   request's tenant (KNL for all existing users).
7. **Platform admin bootstrap.** The global `CONSOLE_COLDSTART_OTP` now seeds the
   PLATFORM admin (sentinel org). If you need a platform login, set that secret +
   redeem the OTP per the onboarding docs. KNL's own admins are unchanged (they
   were backfilled to the KNL org).

---

## 2. Verify live (still in the window)

- `https://console.knllogistic.com` returns 200, valid TLS, app loads (no blank
  ErrorBoundary). Legacy `https://fsm.knllogistic.com` returns a 301 to the
  console host (and still serves the app if the redirect is ever detached).
- Log in as an existing KNL admin (passkey). Confirm work orders, equipment,
  messages, support tickets, dispatch, KPIs are **visible** (RLS-scoped to KNL,
  not empty). Empty here = the GUC isn't being set / role misconfigured → roll
  back.
- Create a trivial audited write (e.g. a test work order) and confirm it persists
  + audits, then clean it up.
- `/ops` (per-tenant ops dashboard) renders KNL's rollups.
- Check app logs for `permission denied` / zero-row anomalies and that requests
  carry tenant context.
- Optional: onboard a throwaway second org via the platform console and confirm
  it cannot see KNL data, then archive it.

Only after these pass: **end the maintenance window** (remove the maintenance
page / confirm replicas are healthy).

---

## 3. Rollback

Pick based on how far the cutover got:

- **Before step 1.3 (migrations not yet applied):** trivial — scale the app back
  up on the OLD digest; nothing changed.
- **After migrations, app verification fails:** the DB now has RLS on, so simply
  reverting the app digest is NOT enough (the old owner-app reads empty). Two
  options:
  1. **PITR restore (preferred, clean):** restore the CNPG cluster to the
     timestamp recorded in step 1.2 (pre-migration). No tenant data was written
     during the window (app was scaled to 0), so this loses nothing. Then revert
     the digest pin. Follow `ops/dr/cnpg-restore-drill.md`.
  2. **Disable-RLS escape hatch (faster, if PITR is slow):** as owner, run a
     prepared script that `ALTER TABLE … NO FORCE ROW LEVEL SECURITY; DISABLE ROW
     LEVEL SECURITY;` on every tenant table (the inverse of 0030/0035), then
     revert the digest so the old owner-app serves again. Keep this script ready
     in `ops/launch/disable-rls-rollback.sql` before the window. (Org_id columns
     and backfill can stay; they are additive and harmless to the old app.)
- Either way, after rollback: announce, then triage the failure on the branch
  before re-attempting.

---

## 4. Post-cutover follow-ups (not blocking the window)

- **Re-enable auto-sync** (reverse of §0.0) once verified, so GitOps drift-
  correction resumes:
  ```sh
  kubectl -n argocd patch application maintenance --type merge \
    -p '{"spec":{"syncPolicy":{"automated":{"prune":true,"selfHeal":true}}}}'
  ```
  (and the root app if you overrode it). From here, routine deploys are normal:
  merge to `main` → image-release signs → bump the prod digest → auto-sync runs
  the idempotent `migrate-job` (a no-op once up to date) then rolls the app.
- Remove the maintenance page; confirm Argo shows the app Healthy/Synced on the
  new digests.
- Confirm the next routine deploy runs the wave-2 Sync `migrate-job` cleanly (it is
  idempotent — "up to date").
- Track: deploy-time signature enforcement (#6), observability stack (#10), the
  refresh/worker org follow-ups, and the `work_orders.service_category` column if
  the team wants 정비 category as queryable data.

---

## Why a window (rationale, for reviewers)

`FORCE ROW LEVEL SECURITY` subjects even the table owner to RLS. The old app
connects as the owner and does not set `app.current_org`, so the moment RLS is
enabled it reads zero rows. The new app connects as `console_rt` and sets the GUC per
request, so it works. There is therefore an unavoidable interval — between
"migrations enable RLS" and "new app is the only one serving" — during which the
old app is broken. Scaling the old app to 0 for the duration removes that
interval from user-visible traffic. The cutover is otherwise data-safe: org_id
backfill is deterministic (everything → KNL), and the PITR target gives a clean
rollback.
