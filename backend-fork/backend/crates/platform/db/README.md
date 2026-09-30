# console-platform-db

Postgres schema migrations (SQLx) and the `with_audit` transactional helper.

## Local setup

```sh
# Start Homebrew PostgreSQL 16
brew services start postgresql@16

# Create the development database (first time only)
createdb console_dev

# Run tests (migrations are applied automatically per #[sqlx::test])
DATABASE_URL=postgres://localhost/console_dev cargo test -p console-platform-db
```

## Offline query cache

The `query!` macros are checked at compile time. The committed `.sqlx/`
directory in the workspace root is the offline cache used by CI
(`SQLX_OFFLINE=true`).

**Regenerate after any schema or query change:**

```sh
DATABASE_URL=postgres://localhost/console_dev cargo sqlx prepare --workspace
```

Then commit the updated `.sqlx/` directory. CI will fail with a clear error
if the cache is stale.

## Migrations

| File | Contents |
|------|----------|
| `0001_create_regions_branches.sql` | `regions`, `branches` tables |
| `0002_create_users.sql` | `users`, `user_branches` tables (5-role matrix) |
| `0003_create_audit_events.sql` | `audit_events` append-only table + trigger defense |

## Append-only invariant

`audit_events` enforces immutability at two independent layers:

1. `REVOKE UPDATE, DELETE ON audit_events FROM PUBLIC` (permission layer).
2. `BEFORE` triggers on `UPDATE` and `DELETE` that raise an exception
   (defense-in-depth — fires even for privileged roles).

The `#[sqlx::test]` suite asserts both layers independently.
