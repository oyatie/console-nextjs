# console-platform-provisioning

Bulk roster provisioning and passkey cold-start for T0.12.

## Roster Format

The crate accepts UTF-8 JSON. Phone is the stable roster identity and is
required for idempotent upsert.

```json
{
  "users": [
    {
      "display_name": "Kim Mechanic",
      "phone": "010-1000-0001",
      "team": "정비",
      "roles": ["MECHANIC"],
      "branches": [
        { "region": "수도권", "branch": "서울" }
      ]
    }
  ]
}
```

Roles must be one or more of `SUPER_ADMIN`, `ADMIN`, `MECHANIC`,
`RECEPTIONIST`, `EXECUTIVE`. Teams must match the existing `users.team` check:
`정비`, `예방`, `관리`, or `접수`.

Branch memberships are reconciled exactly to the roster row. Unknown branches
fail validation and roll back the entire import.
