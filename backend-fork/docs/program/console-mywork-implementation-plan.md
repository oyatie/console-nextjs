# My Work console slice plan

1. Prove the existing `mywork` registry mount and authenticated action-inbox/todo
   adapter with focused tests; do not create another endpoint or client-side data.
2. Make the action queue operationally scannable using only API fields already
   supplied by `ActionInboxItem`: source kind, server urgency, state, exact due
   timestamp, canonical object link, and source metadata.
3. Preserve closed-world deep linking, person-scoped API reads, and empty/loading/
   error recovery; responsive layout must not remove task actions or semantic labels.
4. Update the source-parity ledger only after tests demonstrate the mounted,
   real-data path. This is source integration evidence, not release readiness.
