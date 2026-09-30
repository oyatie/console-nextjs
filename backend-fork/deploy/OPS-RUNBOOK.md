> **UNVERIFIED HISTORICAL OCI RECOVERY NOTES:** The consolidation audit did not
> establish that this topology is currently live, that the laptop kubeconfig
> names the production cluster, or that the listed Vault objects remain complete.
> Do not run commands against the addresses below, mutate OCI/Kubernetes/DNS, or
> rotate credentials until an authorized operator independently re-establishes
> account identity, tenancy/compartment, cluster identity, current topology, and
> change authority. Repository manifests and a passing hardening gate are not live
> observations. See
> [`docs/handoffs/2026-08-03-disk-wipe-consolidation.md`](../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

# OPS Runbook — historical OCI/Talos recovery record (knllogistic.com)

This records how the former operator accessed the declared Talos,
Kubernetes/GitOps, and database topology. It is recovery input, not permission to
take control, and completeness of OCI Vault plus this document is unverified.

This file is the historical **`oci-guest` runbook** for a declared Oracle Cloud
Ampere A1 cluster. The additive ADR-0024 bare-metal/on-prem operator path is documented in
[`OPS-RUNBOOK-baremetal.md`](OPS-RUNBOOK-baremetal.md). Do not copy OCI-only
constraints from this file — the single A1 warning, OCI Vault, OCI Bastion, OCI
Object Storage, the `dd` boot-volume bootstrap, and the OCI MTU workaround — onto
bare metal.

## Deployment-context hardening properties (`oci-guest`)

| Property | Repository-declared historical `oci-guest` posture |
|---|---|
| Secret store | **OCI Vault** was documented as the recovery source for Talos/kubeconfig/app secret bundles. Kubernetes `Secret` objects were created out of band from that bundle; External Secrets / Sealed Secrets were an upgrade path, not a proven current controller. |
| Object-store endpoint and retention | CNPG/Barman uses OCI Object Storage through the S3-compatible endpoint in `deploy/apps/console/base/database.yaml` (`s3://mnt-db-backups/`), with the `oci-objectstore-creds` customer-secret-key pair. The manifest sets `retentionPolicy: "35d"`, so the Barman restorable window is finite; this is not proof of an independent off-site copy. Evidence objects use the same OCI S3-compatible posture until a context-specific endpoint is selected. |
| Database/topology HA | One VM.Standard.A1.Flex node, one schedulable Talos control-plane, and CNPG `instances: 1`. The API/web replicas and PDBs improve rollout behavior but do not survive node loss because they share the same node. |
| Automatic failover | **Not present** for node or database loss in this context. A failed node is a restore-from-backup / rebuild event using Vault plus Barman backup artifacts, not a transparent failover. |

Passing the production-hardening gate for `oci-guest` proves only that repository
artifacts describe a single-node, restore-not-failover posture consistently. It
does not prove a live or recoverable deployment, and it does not establish
multi-node HA.

## 0. The one rule that bit us
**Never keep the only copy of a cluster credential in `/tmp` or a single laptop.**
The former record says cluster secrets lived in **OCI Vault** (`bitween-default-vault`, compartment
`cloud`, region `ap-chuncheon-1`, key `oyatie-cloud-master-key`). Retrieve any with:
```sh
oci vault secret list --compartment-id <cloud-compartment-ocid> --region ap-chuncheon-1
oci secrets secret-bundle get --secret-id <id> --query 'data."secret-bundle-content".content' --raw-output | base64 -d
```
Vault secrets: `console-talos-secrets` (Talos PKI / secrets.yaml — regenerates a
matching talosconfig), `console-talos-kubeconfig` (tar of talosconfig + kubeconfig),
`console-app-secrets-bundle` (tar of JWT ES256 keypair, console_rt DB password, coldstart
OTP, OCI S3 customer-secret-key).

## 1. Last-recorded facts — unverified (region ap-chuncheon-1, prod compartment)
- Node: **console-fsm-node**, VM.Standard.A1.Flex 4 OCPU/24 GB (the ENTIRE free-tier
  A1 allotment — never run a second A1).
- **Reserved public IP `140.245.68.253`** (stable; DNS points here). Private `10.0.0.227`.
- k8s `v1.36.1`, Talos `v1.13.4`, single control-plane node (schedules workloads).
- Cloudflare zone `knllogistic.com` (id `42acb0af77c89c6db60b6878c1eea7e0`), all
  3 A-records (apex/www/fsm) PROXIED → `140.245.68.253`. cert-manager uses **DNS-01**
  (the proxy breaks HTTP-01); needs the `cloudflare-api-token` secret in `cert-manager`.

## 2. Historical kubectl path (do not run before identity and authority checks)
```sh
export KUBECONFIG=/Users/jasonlee/.config/talos-mnt/_talos/kubeconfig   # or restore from Vault console-talos-kubeconfig
kubectl get nodes        # server: https://140.245.68.253:6443
```
Large responses work **only because eth0 is pinned to MTU 1500** (OCI VNICs default
to 9000; the public path drops oversized frames — ICMP frag-needed is filtered, so
PMTUD blackholes). This is baked into `deploy/talos/oci-guest/controlplane.patch.yaml`.

## 3. Historical talosctl path (do not run before identity and authority checks)
```sh
export TALOSCONFIG=/Users/jasonlee/.config/talos-mnt/_talos/talosconfig
talosctl -e 140.245.68.253 -n 140.245.68.253 <cmd>
```
If apid (`:50000`) is flaky over the public IP, tunnel via the **OCI Bastion service**
(bastion `ocid1.bastion.oc1.ap-chuncheon-1.amaaaaaax62ibfya5kxkkxtbrrd5xnxx4yn4mlj4ir3rwicy6y4im7sqyhua`):
```sh
oci bastion session create-port-forwarding --bastion-id <bastion> --ssh-public-key-file key.pub \
  --target-private-ip 10.0.0.227 --target-port 50000 --session-ttl 10800 --wait-for-state SUCCEEDED
# then: ssh -i key -N -L 50000:10.0.0.227:50000 -p 22 <session-ocid>@host.bastion.ap-chuncheon-1.oci.oraclecloud.com
talosctl config endpoint 127.0.0.1 && talosctl config node 10.0.0.227 && talosctl <cmd>
```

## 4. Database (CloudNativePG, ns `maintenance`)
- Cluster `console-db`, pod `console-db-1` (2/2). Owner role `console_app` (secret `console-db-app`,
  auto-generated), runtime `console_rt` (secret `console-db-rt`, you create; RLS-bound).
```sh
kubectl exec -n console console-db-1 -c postgres -- psql -U postgres -d console -c '\dt'
```
- Migrations run as an Argo **PreSync hook** `console-migrate` (console-app image, CONSOLE_APP_ROLE=migrate).
- **KNOWN fresh-deploy DB gotchas** (apply in order on a clean cluster; each has a
  proper fix to land so the next rebuild is hands-off):
  1. **DB-before-migrate deadlock.** migrate (PreSync) needs `console-db-app`, but the
     `console-db` Cluster is a regular Sync resource → created *after* the hook. Bootstrap
     the DB first: `kubectl apply --server-side -f` the Cluster + ObjectStore from the
     prod render. *Proper fix:* a pre-migrate sync-wave / separate bootstrap app.
  2. **Migration role needs BYPASSRLS.** `organizations`/`users` have FORCE RLS (0030),
     so 0034's FK validation — run as `console_app` (subject to RLS, no `app.current_org`) —
     sees zero orgs and the FK looks violated. `ALTER ROLE console_app BYPASSRLS;` (only the
     migration role; runtime `console_rt` STAYS NOBYPASSRLS = the real tenant boundary).
     *Proper fix:* CNPG `managed.roles` set `console_app` bypassrls.
  3. **0033 backfill order.** It backfills `auth_bootstrap_credentials.org_id` from its
     user, but before the user's own org backfill → the cold-start cred stays NULL →
     0034 NOT-NULL fails:
     `UPDATE auth_bootstrap_credentials c SET org_id=u.org_id FROM users u WHERE u.id=c.user_id AND c.org_id IS NULL;`
     *Proper fix:* reorder 0033 (users first) or give the cred a default-org backfill.
  4. **console-db-rt password must be URL-encoded** in the `uri`. `openssl rand -base64`
     yields `+`/`/`, which break `host:port` parsing → backend crashes
     `Database(Configuration(InvalidPort))`. Percent-encode the password in the uri.
     *Proper fix:* SECRETS.md uses `rand -hex` or percent-encodes.
  5. **Apalis schema ownership is migration-only.** The intended application shape
     now runs the vendor Apalis migrations and reconciles the queue ACL in
     `CONSOLE_APP_ROLE=migrate` as owner `console_app`, on the same validated physical
     connection as the numbered SQLx migrations. API/worker startup as `console_rt`
     performs read-only schema, ledger/checksum, and ACL validation and fails closed
     on drift; it does not self-create queue objects or require runtime DDL grants.
     This describes the merge candidate, not proof that the live cluster has
     activated or successfully exercised the new boundary. Existing databases
     may still have `console_rt`-owned Apalis objects, the legacy migration ledger,
     or the vendor `public.generate_ulid()` helper. Activation remains blocked
     until an administrator inventories those exact objects, rehearses an
     enumerated owner transfer on a restored disposable copy, and proves the
     migrate-to-runtime transition. Never use broad `REASSIGN OWNED BY console_rt`;
     the governed component runbook defines the required evidence boundary.
- **Restoring from a dev/local dump:** purge dev-auth role-switch personas before pointing
  a release build at it — `DELETE FROM users WHERE phone LIKE 'dev-auth:%';`. The
  composition root refuses to boot (api/worker) if any remain (`console-app`'s
  `assert_no_dev_auth_personas`, compiled out only under `--features dev-auth`).

### PR 473 governed command topology is not live

The historical OCI record assumes the two-role and PreSync behavior above; this
was not re-observed during consolidation. PR 473's
six-role topology, ordered Sync waves, command credentials, connection budget,
rotation procedure, recovery bundle, and rollback evidence are held in the
unreferenced
[`governed-command-database` component runbook](apps/console/components/governed-command-database/README.md).
Do not apply that component to this cluster, selectively sync its migration or
serving resources, or treat a successful render as a live readiness claim.

## 4.5. Dark mox mail stack (ns `maintenance`)

- Workload: `statefulset/console-mox` (single replica) with PVC
  `mox-data-console-mox-0` mounted at `/mox-data`. The image is
  `r.xmox.nl/mox@sha256:47497222e83679f95049329f12c5d8c4bfd3b809e62d4ffcfd508907e66b06a5`.
  mox must start as root and then drops to UID/GID 10001 from `mox.conf`; the
  container keeps `allowPrivilegeEscalation=false`, grants only the ownership and
  setuid/setgid capabilities needed for that drop, and
  exposes only ClusterIP ports 1080 (webapi), 1143 (plain IMAP, internal-only),
  and 8010 (Prometheus metrics). There is no Ingress, hostPort, NodePort,
  LoadBalancer, SMTP/25, submission, IMAPS, webmail, or admin interface in the
  dark deployment.
- App wiring: `console-config` sets
  `CONSOLE_MAIL_MOX_BASE_URL=http://console-mox.console.svc:1080`; the app/worker
  read `CONSOLE_MAIL_MOX_WEBHOOK_SECRET` from `console-secrets`. HTTP is intentionally
  service-local and protected by NetworkPolicy; do not switch it to HTTPS unless
  the backend reqwest rustls feature/test path is updated in the same PR.
- Initial mox config: on first boot the pod copies `console-mox-bootstrap` into the
  PVC and renders `domains.conf` with the webhook Authorization value. On later
  pod starts it refreshes only that `Authorization: Bearer ...` line from
  `console-secrets`, so secret rotation propagates without overwriting other mox
  config/admin changes on the PVC; validate edits with:

  ```sh
  kubectl exec -n console statefulset/console-mox -- /bin/mox -config /mox-data/config/mox.conf config test
  ```

  Webhook-secret rotation: update `console-secrets`, then restart every consumer so
  mox rewrites the bearer line and the api/worker reload the same new value.
  Keep shell tracing off and verify with config test plus the dark smoke; do not
  print `domains.conf` or secret env values.

  ```sh
  kubectl -n console rollout restart statefulset/console-mox
  kubectl argo rollouts restart console-app -n console
  kubectl -n console rollout restart deployment/console-worker
  kubectl -n console rollout status statefulset/console-mox --timeout=300s
  kubectl argo rollouts status console-app -n console --timeout 300s
  kubectl -n console rollout status deployment/console-worker --timeout=300s
  kubectl exec -n console statefulset/console-mox -- \
    /bin/mox -config /mox-data/config/mox.conf config test
  # Then run the dark smoke below using the rotated Vault/console-secrets values.
  ```

- Bootstrap account password: retrieve the operator-held postmaster/account
  password from OCI Vault (see `SECRETS.md`) and set it over the in-pod control
  socket after the pod is Ready:

  ```sh
  set -euo pipefail
  set +x
  MOX_PASS_SECRET_OCID="${MOX_PASS_SECRET_OCID:?set to the console-mox-postmaster-password OCI Vault secret OCID}"
  MOX_PASS="$(oci secrets secret-bundle get --secret-id "$MOX_PASS_SECRET_OCID" \
    --query 'data."secret-bundle-content".content' --raw-output | base64 -d)"
  test -n "$MOX_PASS"
  printf '%s' "$MOX_PASS" | kubectl exec -i -n console statefulset/console-mox -- \
    /bin/mox -config /mox-data/config/mox.conf setaccountpassword postmaster
  unset MOX_PASS MOX_PASS_SECRET_OCID
  ```

  `setaccountpassword` reads stdin and stores derived verifier material in
  `/mox-data`; never paste, echo, or log the secret in shell history.
- NetworkPolicy: `default-deny-ingress` already covers the namespace.
  `allow-app-egress-mox` permits only app/worker → mox webapi/IMAP;
  `allow-mox-ingress-internal` permits only app/worker (webapi/IMAP) and the
  `monitoring` namespace (metrics); `default-deny-egress-mox` plus DNS and
  `allow-mox-egress-app-webhook` keep mox egress to DNS and the internal app
  webhook. Plain flannel does not enforce this — record a live CNI smoke before
  claiming runtime enforcement.
- Static render/policy proof:

  ```sh
  scripts/check-networkpolicy-enforcement.sh
  kustomize build deploy/apps/console/overlays/prod >/tmp/console-prod.yaml
  ```

- Dark smoke without public MX:

  ```sh
  set -euo pipefail
  set +x
  CONSOLE_MOX_PF_PID=""
  CONSOLE_APP_PF_PID=""
  cleanup_mox_e2e() {
    [ -z "${CONSOLE_MOX_PF_PID:-}" ] || kill "$CONSOLE_MOX_PF_PID" 2>/dev/null || true
    [ -z "${CONSOLE_APP_PF_PID:-}" ] || kill "$CONSOLE_APP_PF_PID" 2>/dev/null || true
    unset MOX_PASS WEBHOOK_SECRET MOX_PASS_SECRET_OCID CONSOLE_MOX_PF_PID CONSOLE_APP_PF_PID
  }
  trap cleanup_mox_e2e EXIT
  MOX_PASS_SECRET_OCID="${MOX_PASS_SECRET_OCID:?set to the console-mox-postmaster-password OCI Vault secret OCID}"
  MOX_PASS="$(oci secrets secret-bundle get --secret-id "$MOX_PASS_SECRET_OCID" \
    --query 'data."secret-bundle-content".content' --raw-output | base64 -d)"
  WEBHOOK_SECRET="$(kubectl -n console get secret console-secrets \
    -o jsonpath='{.data.CONSOLE_MAIL_MOX_WEBHOOK_SECRET}' | base64 -d)"
  test -n "$MOX_PASS"
  test -n "$WEBHOOK_SECRET"
  kubectl -n console port-forward svc/console-mox 1080:1080 >/tmp/console-mox-port-forward.log 2>&1 &
  CONSOLE_MOX_PF_PID=$!
  kubectl -n console port-forward svc/console-app 8090:8080 >/tmp/console-app-port-forward.log 2>&1 &
  CONSOLE_APP_PF_PID=$!
  for url in http://127.0.0.1:1080/webapi/v0/ http://127.0.0.1:8090/readyz; do
    for i in $(seq 1 30); do curl -fsS "$url" >/dev/null && break || sleep 1; done
    curl -fsS "$url" >/dev/null
  done
  CONSOLE_MOX_WEBAPI_URL=http://127.0.0.1:1080 \
  CONSOLE_DEV_BACKEND_URL=http://127.0.0.1:8090 \
  CONSOLE_MOX_USER=postmaster@knllogistic.com \
  CONSOLE_MOX_PASS="$MOX_PASS" \
  CONSOLE_MAIL_MOX_WEBHOOK_SECRET="$WEBHOOK_SECRET" \
  node scripts/mox-e2e.mjs
  ```

- Backup/restore: CNPG/Barman covers Postgres only; it does **not** back up
  `/mox-data`. Before rollout, destructive changes, or PVC deletion, run mox's
  file-level backup into secure scratch directories, encrypt the tarball before
  upload, store it in an encrypted OCI Object Storage bucket with explicit
  retention/quota approval, verify the uploaded object, and only then remove pod
  and local scratch copies:

  ```sh
  set -euo pipefail
  set +x
  umask 077
  STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
  POD_BACKUP="/tmp/mox-backup-$STAMP"
  LOCAL_BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mox-backup.XXXXXX")"
  trap 'echo "backup failed; preserving pod path $POD_BACKUP and local dir $LOCAL_BACKUP_DIR" >&2' ERR
  ARCHIVE="$LOCAL_BACKUP_DIR/mox-backup-$STAMP.tgz"
  ENC_ARCHIVE="$ARCHIVE.enc"
  BACKUP_ENC_SECRET_OCID="${BACKUP_ENC_SECRET_OCID:?set to the mox backup encryption passphrase secret OCID}"
  BACKUP_MAC_SECRET_OCID="${BACKUP_MAC_SECRET_OCID:?set to the mox backup HMAC secret OCID}"
  CONSOLE_MOX_BACKUP_BUCKET="${CONSOLE_MOX_BACKUP_BUCKET:?set to the approved encrypted retention bucket}"
  BACKUP_ENC_PASS="$(oci secrets secret-bundle get --secret-id "$BACKUP_ENC_SECRET_OCID" \
    --query 'data."secret-bundle-content".content' --raw-output | base64 -d)"
  BACKUP_MAC_KEY="$(oci secrets secret-bundle get --secret-id "$BACKUP_MAC_SECRET_OCID" \
    --query 'data."secret-bundle-content".content' --raw-output | base64 -d)"
  test -n "$BACKUP_ENC_PASS"
  test -n "$BACKUP_MAC_KEY"

  kubectl exec -n console statefulset/console-mox -- /bin/sh -ceu '
    umask 077
    rm -rf "$1"
    mkdir -m 700 "$1"
    /bin/mox -config /mox-data/config/mox.conf backup "$1"
  ' sh "$POD_BACKUP"
  kubectl cp "maintenance/console-mox-0:$POD_BACKUP" "$LOCAL_BACKUP_DIR/mox-backup"
  tar -C "$LOCAL_BACKUP_DIR/mox-backup" -czf "$ARCHIVE" .
  openssl enc -aes-256-cbc -pbkdf2 -salt -in "$ARCHIVE" -out "$ENC_ARCHIVE" \
    -pass env:BACKUP_ENC_PASS
  python3 - "$ENC_ARCHIVE" "$ENC_ARCHIVE.hmac" <<'PY'
  import base64, hashlib, hmac, os, sys
  key = os.environ["BACKUP_MAC_KEY"].encode()
  with open(sys.argv[1], "rb") as src:
      mac = hmac.new(key, src.read(), hashlib.sha256).digest()
  with open(sys.argv[2], "w", encoding="utf-8") as dst:
      dst.write(base64.b64encode(mac).decode() + "\n")
  PY
  (cd "$LOCAL_BACKUP_DIR" && shasum -a 256 "$(basename "$ENC_ARCHIVE")" > "$(basename "$ENC_ARCHIVE").sha256")
  oci os object put --bucket-name "$CONSOLE_MOX_BACKUP_BUCKET" \
    --name "mox/$STAMP/$(basename "$ENC_ARCHIVE")" --file "$ENC_ARCHIVE"
  oci os object put --bucket-name "$CONSOLE_MOX_BACKUP_BUCKET" \
    --name "mox/$STAMP/$(basename "$ENC_ARCHIVE").sha256" --file "$ENC_ARCHIVE.sha256"
  oci os object put --bucket-name "$CONSOLE_MOX_BACKUP_BUCKET" \
    --name "mox/$STAMP/$(basename "$ENC_ARCHIVE").hmac" --file "$ENC_ARCHIVE.hmac"
  oci os object get --bucket-name "$CONSOLE_MOX_BACKUP_BUCKET" \
    --name "mox/$STAMP/$(basename "$ENC_ARCHIVE")" --file "$LOCAL_BACKUP_DIR/verify.enc"
  test "$(shasum -a 256 "$LOCAL_BACKUP_DIR/verify.enc" | awk '{print $1}')" = \
    "$(cut -d ' ' -f1 "$ENC_ARCHIVE.sha256")"
  python3 - "$LOCAL_BACKUP_DIR/verify.enc" "$LOCAL_BACKUP_DIR/verify.enc.hmac" <<'PY'
  import base64, hashlib, hmac, os, sys
  key = os.environ["BACKUP_MAC_KEY"].encode()
  with open(sys.argv[1], "rb") as src:
      mac = hmac.new(key, src.read(), hashlib.sha256).digest()
  with open(sys.argv[2], "w", encoding="utf-8") as dst:
      dst.write(base64.b64encode(mac).decode() + "\n")
  PY
  cmp -s "$LOCAL_BACKUP_DIR/verify.enc.hmac" "$ENC_ARCHIVE.hmac"
  kubectl exec -n console statefulset/console-mox -- rm -rf "$POD_BACKUP"
  rm -rf "$LOCAL_BACKUP_DIR"
  trap - ERR
  unset BACKUP_ENC_PASS BACKUP_MAC_KEY BACKUP_ENC_SECRET_OCID BACKUP_MAC_SECRET_OCID CONSOLE_MOX_BACKUP_BUCKET
  ```

  Restore drill: prove this on a fresh PVC/workload when possible; for an
  in-place restore, scale mox down first and do **not** delete the old PVC or
  backup tarball until every validation below passes.

  ```sh
  BACKUP=./mox-backup-YYYYMMDDTHHMMSSZ.tgz
  kubectl -n console scale statefulset/console-mox --replicas=0
  kubectl -n console wait --for=delete pod/console-mox-0 --timeout=180s
  ```

  Mount the restore target PVC with a one-shot root pod. For a fresh-PVC drill,
  create the PVC first and replace `claimName: mox-data-console-mox-0` below with
  that claim instead of the production claim.

  ```sh
  cat >/tmp/console-mox-restore-pod.yaml <<'YAML'
  apiVersion: v1
  kind: Pod
  metadata:
    name: console-mox-restore
    namespace: console
  spec:
    restartPolicy: Never
    securityContext: { runAsUser: 0, fsGroup: 10001 }
    containers:
      - name: restore
        image: r.xmox.nl/mox@sha256:47497222e83679f95049329f12c5d8c4bfd3b809e62d4ffcfd508907e66b06a5
        command: ["/bin/sh", "-ceu", "sleep 3600"]
        volumeMounts:
          - { name: mox-data, mountPath: /mox-data }
    volumes:
      - name: mox-data
        persistentVolumeClaim: { claimName: mox-data-console-mox-0 }
  YAML
  kubectl apply -f /tmp/console-mox-restore-pod.yaml
  kubectl -n console wait --for=condition=Ready pod/console-mox-restore --timeout=120s
  kubectl -n console cp "$BACKUP" console-mox-restore:/tmp/mox-restore.tgz
  kubectl -n console exec console-mox-restore -- /bin/sh -ceu '
    rm -rf /mox-data/* /mox-data/.[!.]* /mox-data/..?*
    tar -C /mox-data -xzf /tmp/mox-restore.tgz
    chown -R 10001:10001 /mox-data
    chmod 700 /mox-data /mox-data/config /mox-data/data
    test -s /mox-data/config/mox.conf
    test -s /mox-data/config/domains.conf
    /bin/mox -config /mox-data/config/mox.conf config test
  '
  kubectl -n console delete pod/console-mox-restore
  ```

  Start mox only after the restored files/config validate, then prove the live
  webapi and dark smoke before removing the old PVC/export copy.

  ```sh
  kubectl -n console scale statefulset/console-mox --replicas=1
  kubectl -n console rollout status statefulset/console-mox --timeout=300s
  kubectl -n console exec statefulset/console-mox -- \
    /bin/mox -config /mox-data/config/mox.conf config test
  kubectl -n console port-forward svc/console-mox 1080:1080
  curl -fsS http://127.0.0.1:1080/webapi/v0/ >/dev/null
  # In a second terminal, port-forward svc/console-app 8090:8080 and run the dark smoke:
  # node scripts/mox-e2e.mjs with CONSOLE_MOX_* and CONSOLE_MAIL_MOX_WEBHOOK_SECRET from Vault/console-secrets.
  ```

- Observability: enable `deploy/apps/console/components/monitoring` only when
  Prometheus Operator CRDs exist. Scrape `svc/console-mox:8010/metrics` and alert on
  `ConsoleMoxDown`, `ConsoleMoxWebhookFailures`, `ConsoleMoxQueueBacklog`, and
  `ConsoleMoxPvcSaturation`. Keep mox `LogLevel: info`; never enable `traceauth` or
  `tracedata` in production because those can expose credentials or full message
  bodies.
- Rollback: revert the Git commit/config, Argo sync, and remove or disable
  `CONSOLE_MAIL_MOX_BASE_URL` for both api and worker consumers if app webmail
  traffic must stop using mox. Restart both workloads and verify neither still
  sees the mox base URL before scaling down `console-mox`.

  ```sh
  kubectl argo rollouts restart console-app -n console
  kubectl -n console rollout restart deployment/console-worker
  kubectl argo rollouts status console-app -n console --timeout 300s
  kubectl -n console rollout status deployment/console-worker --timeout=300s
  APP_POD="$(kubectl -n console get pods -l app=console-app -o jsonpath='{.items[0].metadata.name}')"
  WORKER_POD="$(kubectl -n console get pods -l app=console-worker -o jsonpath='{.items[0].metadata.name}')"
  test -n "$APP_POD"
  test -n "$WORKER_POD"
  ! kubectl -n console exec "$APP_POD" -- printenv CONSOLE_MAIL_MOX_BASE_URL
  ! kubectl -n console exec "$WORKER_POD" -- printenv CONSOLE_MAIL_MOX_BASE_URL
  ```

  Scale `console-mox` to 0 only after a successful backup/export; do not delete the
  PVC unless the restore path above has been proven.
- Public MX/operator gate: public SMTP/MX, submission, IMAPS, webapi, or admin UI
  must remain disabled until DNS MX/SPF/DKIM/DMARC/MTA-STS/TLS-RPT, TLS/cert
  rotation, queue/dead-letter behavior, abuse/rate-limit/open-relay negatives,
  monitoring/alerts, backup/restore, rollback, OCI firewall/security-list
  posture, and real client-IP visibility for junk filtering and rate-limiting are
  all proven and explicitly approved. Acceptable client-IP preservation paths
  include host networking, `externalTrafficPolicy: Local`, proxy protocol, or a
  different ingress path that mox can verify before public port 25/submission is
  enabled.

## 5. The GitOps server (Argo CD, ns `argocd`)
- Argo watches branch **main**, app-of-apps
  `root` → cert-manager, traefik, cnpg-operator, barman-plugin, local-path,
  argo-rollouts, cluster-issuer, **maintenance** (the app + DB + ingress).
- Install (if rebuilding): `kubectl apply --server-side --force-conflicts -n argocd -k
  https://github.com/argoproj/argo-cd/manifests/cluster-install?ref=v3.4.3` (server-side
  REQUIRED — the applicationsets CRD exceeds the 256 KB client-side annotation limit).
- Non-git secrets to create first: see `deploy/SECRETS.md` (`console-secrets`,
  `oci-objectstore-creds`, `console-db-rt` in `maintenance`; `cloudflare-api-token` in
  `cert-manager`). Material is in Vault `console-app-secrets-bundle`.
- App images: main build (`gh workflow run image-release.yml --ref main`),
  digests pinned in `deploy/apps/console/overlays/prod/kustomization.yaml`.

## 6. Verified deploy versus digest-bump-only

`scripts/deploy.sh` is the operator-facing output contract for production deploys.
Run it from the repo root with a pushed commit SHA after CI/Image Release has
built the images:

```sh
scripts/deploy.sh <git-sha>
```

Default mode is fail-closed and is the only mode that may be described as
"deployed and verified". It requires `gh`, `git`, `curl`, `kubectl`, the
argo-rollouts kubectl plugin, and a kubeconfig pointed at the target cluster. The
script must reach its final `done: <sha> deployed and verified (...)` line before
an operator or release note claims deployment completion. The required signals are:

1. the matching `image-release.yml` run for the commit succeeds;
2. the `digest-console-app` artifact provides a fresh `sha256` digest;
3. the prod kustomization pins those digests and the bump commit/revision is the
   desired GitOps revision;
4. Argo Application `maintenance` reports `Synced` at that revision;
5. the `console-app` Argo Rollout becomes Healthy;
6. `console-worker` Deployment rollout completes;
7. each workload template image and each running/ready pod's image ID or image
   reference matches the built digest; and
8. `https://console.knllogistic.com` and `https://knllogistic.com` return HTTP
   200.

If `kubectl` is missing, the kubeconfig cannot reach the target cluster, the Argo
Application cannot be read/refreshed, a rollout fails, a pod is not ready, a digest
does not match, or an endpoint check fails, the script exits non-zero before the
success line. Treat that as **not deployed/verified** and fix access or the
cluster state; do not downgrade the claim to a partial success.

`scripts/deploy.sh --digest-bump-only <git-sha>` (alias: `--bump-only`) is a
separate, explicit desired-state operation for hosts without production cluster
access. It updates/commits the prod digest pins and exits with
`done: <sha> desired prod digests updated only (...); deployment, rollout,
pod-image, and endpoint verification were NOT run.` Record that exactly as a
digest bump or desired-state update. It is not proof that Argo synced, pods run
the new images, endpoints serve traffic, or go-live readiness changed. A second
operator with cluster access must still run the default deploy path before any
deployment-complete or production-ready statement.

## 7. Full rebuild from zero
The former procedure says Talos was `dd`'d onto the boot volume and read config
from gzip-compressed user data. It also delegated the alleged complete sequence
to ephemeral AI memory and untracked scripts under
`/Users/jasonlee/.config/talos-mnt/`. That is not a durable or currently tested
rebuild procedure. The local scripts and Talos/kubeconfig material are explicit
encrypted external-custody items in the disk-wipe handoff; they must never be
copied into Git with credentials. Do not attempt a rebuild until an authorized
operator has recovered and reviewed that bundle, re-established the OCI topology,
and independently proved backup/DNS availability. Repository references to Barman
or object-store buckets do not prove that their data is recoverable.

## 8. Free-tier guardrails
1 A1 node (4 OCPU) · ≤200 GB block · ≤20 GB object · 1 reserved IP (assigned).
Check: `oci compute instance list`, `oci bv boot-volume list`, `oci os object list`.
Delete failed custom images + console-history captures; never leave a 2nd A1 running.
