# DARK warm-standby app group

Generated roots for clusters labeled `console.io/dr-role=warm-standby` point
at this directory after the DARK ApplicationSet is explicitly activated.

It is intentionally empty in the manifest lane so federation can be reviewed
without accidentally syncing the current write-active maintenance overlay to a
standby cluster. Add only standby-safe Applications here after the storage,
secrets, traffic, rollback, and failover runbook lanes approve them.
