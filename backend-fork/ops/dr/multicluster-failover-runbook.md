# Multicluster primary-to-warm-standby failover runbook

Status: DARK / future activation. This runbook applies only after the
ApplicationSet federation package under `deploy/apps/appset-federation/` is
operator-approved and at least one primary plus one warm-standby cluster are
registered with Argo CD.

This document does not activate federation and does not grant permission to move
production traffic. It records the promotion steps the activation ticket must
copy, verify, and evidence during a planned failover rehearsal or an incident.

## Scope

Use this runbook when a Maintenance site has:

- one Argo CD cluster Secret labeled `console.io/dr-role=primary`;
- one or more cluster Secrets labeled `console.io/dr-role=warm-standby`;
- matching or explicitly approved `console.io/residency` labels;
- a documented database/object-storage restore or replication path; and
- a DNS/VIP/ingress owner ready to move traffic after health checks pass.

If the ADR-0024 roadmap item #6 owner is explicitly acting as the broader
failover-orchestration owner for the release, coordinate with that owner before
promotion. ADR-0024 roadmap item #9 owns context-aware CI hardening, not
failover orchestration; assigning that gate does not silently assign promotion
authority. Otherwise this promotion remains founder/operator gated and must not
be delegated implicitly.

## Bare-metal on-prem VIP ingress addendum

For ADR-0024 roadmap item #6, the DNS/VIP/ingress owner must use the dark
activation guide in `deploy/apps/vip-ingress/README.md` before this runbook moves
traffic to an on-prem primary. That path stages MetalLB L2 as the VIP provider
and the sibling `deploy/apps/traefik-onprem/` Traefik variant as a multi-replica
`Service type=LoadBalancer` consumer.

The current OCI guest path remains separate: `deploy/apps/traefik-oci-guest/` and
the live `deploy/argocd/apps/traefik.yaml` preserve reserved IP
`140.245.68.253`, the Traefik `DaemonSet`, disabled Service, and node hostPorts
80/443. Do not treat activating the on-prem VIP as permission to delete or edit
the OCI hostPort path.

Before promotion, record these VIP prerequisites in the incident/rehearsal log:

1. The selected Kubernetes and Argo CD contexts are the intended `on-prem` target,
   not the OCI guest cluster.
2. At least two schedulable workers are on the L2 segment that will advertise the
   ingress VIP.
3. `deploy/apps/vip-ingress/manifests/metallb-l2-config.yaml` has the real
   reserved on-prem VIP or pool instead of the documentation-only
   `10.0.0.240/32` placeholder, and it does not contain `140.245.68.253`.
4. Any `interfaces` or `nodeSelectors` on the `console-onprem-l2`
   `L2Advertisement` match the real worker NICs/labels.
5. `vip-ingress-metallb-onprem` and `traefik-onprem` render and are manually
   applied/synced only after operator approval; neither is under
   `deploy/argocd/apps/` or automated by the app-of-apps root.
6. The `traefik` Service receives the reserved VIP from the
   `console-onprem-ingress` pool, and a health check through the VIP succeeds
   before DNS/upstream routing moves.

Validate VIP failover before claiming the on-prem ingress path is production
ready:

1. Identify the node currently holding or announcing the VIP. Use MetalLB speaker
   logs and/or an L2 ARP probe from the operator network, then match the observed
   MAC to a worker NIC:
   ```sh
   VIP=<reserved-on-prem-vip>
   HOST=<ingress-hostname>
   IFACE=<operator-l2-interface>

   kubectl -n metallb-system get pods -l app.kubernetes.io/component=speaker -o wide
   kubectl -n metallb-system logs -l app.kubernetes.io/component=speaker --since=10m | grep "$VIP" || true
   arping -I "$IFACE" -c 3 "$VIP"
   curl -fsS --resolve "$HOST:443:$VIP" "https://$HOST/healthz"
   ```
2. Kill, reboot, power-isolate, or otherwise remove the VIP holder from service.
   A plain `kubectl drain` is acceptable only when the site's L2 advertisement or
   nodeSelector policy removes that node from VIP eligibility; otherwise the
   MetalLB speaker DaemonSet can keep advertising from a drained node and the
   drill is not valid.
3. Verify the VIP moves to another worker and ingress remains reachable:
   ```sh
   kubectl -n traefik get deploy,pod,svc -o wide
   kubectl -n metallb-system get pods -l app.kubernetes.io/component=speaker -o wide
   arping -I "$IFACE" -c 5 "$VIP"
   for i in $(seq 1 30); do
     date -u +%FT%TZ
     curl -fsS --resolve "$HOST:443:$VIP" "https://$HOST/healthz" || exit 1
     sleep 2
   done
   ```
4. Success signals: a different worker answers ARP/NDP or MetalLB logs show a new
   speaker, the Traefik Service still shows the reserved VIP, at least one
   Traefik pod remains Ready on a surviving worker, repeated HTTPS checks succeed
   after the ARP convergence window, and Argo CD reports the VIP and Traefik apps
   Healthy/Synced.
5. Troubleshoot failures before moving traffic: check the Service annotation names
   `console-onprem-ingress`, the pool has `autoAssign: false`, worker labels
   and L2 interfaces match the advertisement, NetworkPolicy/Cilium permits
   ingress, `externalTrafficPolicy: Local` has local endpoints on the advertising
   node, and cert-manager/Ingress resources are Healthy.

Rollback for this addendum: if activation fails before traffic moves, leave
DNS/upstream routing on the existing target and delete or unsync the manually
applied dark `traefik-onprem` and `vip-ingress-metallb-onprem` apps only after
capturing evidence. If traffic already moved, restore DNS/upstream routing to the
previous target first; for the OCI guest target that means the preserved
hostPort/reserved-IP overlay, not the on-prem VIP files.

## Promotion pre-flight

1. Declare the event type: planned rehearsal, planned cutover, or incident.
2. Assign roles: incident commander, Argo CD operator, database/storage operator,
   DNS/VIP operator, and recorder.
3. Identify the current primary and candidate standby from Argo CD cluster Secret
   labels. Record Secret names, sites, roles, traffic labels, and residency.
4. Freeze writes on the current primary unless the storage layer has already
   proven single-writer promotion safety. For an incident, record whether writes
   may have been lost or split.
5. Verify the candidate standby is allowed by residency policy. If the standby's
   `console.io/residency` conflicts with the primary and no explicit policy
   exception exists, stop and fail closed.
6. Verify cluster credentials are fresh and sourced from External-Secrets/OpenBao
   or the repo-approved secret-management path. Do not paste or log credentials.
7. Verify database/object-storage readiness:
   - for restore-based failover, run or reference the latest successful CNPG/PITR
     restore evidence and choose the recovery target;
   - for replicated storage, prove the standby has a consistent promoted primary
     and no competing writer;
   - for object storage/evidence data, verify the endpoint/bucket configured for
     the standby is current enough for the declared RPO.
8. Verify the standby app group is safe to sync. Standby workloads must not serve
   production traffic until traffic is moved deliberately.
9. For an on-prem target, complete the VIP ingress addendum above and attach the
   VIP holder, failover, ingress health, and rollback evidence to the
   incident/rehearsal record before traffic moves.

## Promotion procedure

1. Announce the promotion start and freeze any remaining deploy automation that
   could race the operation.
2. Promote or restore the data plane first. For CNPG restore drills, use
   `ops/dr/cnpg-restore-drill.md` as the evidence pattern; for live restore, use
   the incident-specific recovery manifest/runbook and record the exact target.
3. Confirm the promoted database is writable only from the intended new primary
   path and that the old primary cannot still accept writes.
4. Update Argo CD cluster Secret labels through the approved secret-management
   path, not by committing Secret payloads:
   - old primary: set `console.io/traffic=held` and either
     `console.io/dr-role=warm-standby` or remove
     `console.io/federation=enabled` while it is quarantined;
   - new primary: set `console.io/dr-role=primary` and
     `console.io/traffic=active`;
   - all other standbys: keep `console.io/dr-role=warm-standby` and
     `console.io/traffic=held`.
5. Reconcile the ApplicationSet. Verify exactly one generated root for the site
   sources `deploy/argocd/apps` at `targetRevision: main`; standby roots must
   source `deploy/apps/appset-federation/app-groups/warm-standby`.
6. Sync the new primary root and wait for the Maintenance API, worker, web, DB,
   ingress, cert-manager, and audit-critical apps to become Healthy/Synced.
7. Run live checks before traffic moves:
   - `/readyz` and `/healthz` pass on the new primary path;
   - login/passkey and a safe audited read work for the expected tenant;
   - a controlled write is either blocked by maintenance mode or succeeds and is
     visible in audit, depending on the incident plan;
   - no old-primary workload is still serving writes;
   - Argo reports no unexpected prune of standby-only resources.
8. Move DNS/VIP/GeoDNS traffic only after step 7 passes and the DNS/VIP operator
   records TTLs, VIP holder, failover-validation result, and rollback target. For
   on-prem VIP ingress, this includes the ADR-0024 roadmap item #6 MetalLB/Traefik
   validation from the addendum above.
9. Keep the old primary quarantined until data divergence is assessed. Do not
   relabel it back into federation as standby until it is rebuilt, restored, or
   explicitly declared safe.
10. Record final evidence: timestamps, labels before/after, ApplicationSet render
    result, Argo health/sync states, DB promotion evidence, traffic move evidence,
    user-facing smoke checks, rollback decision, and any follow-up cards.

## Rollback guidance

- If promotion fails before traffic moves, revert labels to the old primary only
  if it still has the authoritative data state. Delete or suspend the generated
  new-primary root if it was synced.
- If traffic moved but no writes were accepted on the new primary, move traffic
  back, restore the old labels, reconcile ApplicationSet, and record the failed
  check.
- If writes may have occurred on both sides, stop label rollback and invoke the
  database/object-storage recovery owner. Prevent split brain first; user-facing
  recovery waits for the data decision.
- If the old primary was damaged, keep it out of federation by removing
  `console.io/federation=enabled` until rebuilt.

## Drill evidence checklist

Each rehearsal or incident must capture:

- current primary Secret name and labels;
- selected standby Secret name and labels;
- residency policy decision;
- data-plane restore/replication evidence;
- generated primary and standby root names;
- `targetRevision: main` confirmation for the generated primary root;
- Argo Healthy/Synced states after reconciliation;
- DNS/VIP traffic move evidence or explicit N/A reason;
- for on-prem VIP ingress, the pre-move VIP holder, post-failure VIP holder,
  failed/killed/drained node, repeated ingress health checks through the VIP, and
  final MetalLB/Traefik Argo Healthy/Synced state;
- rollback path selected and whether it was tested; and
- follow-up cards for any manual step that should become automated.
